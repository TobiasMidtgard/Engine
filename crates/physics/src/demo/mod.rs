//! The physics proving ground: a 70 × 50 m concrete yard showing what the rigid-body module
//! does. `games/showcase` places it in its physics zone; on its own:
//!
//! ```sh
//! cargo run -p engine_physics --release --example proving_ground -- out_dir
//! ```
//!
//! Around `origin` (x right, z toward the default camera), back row to front row:
//!
//! * a 30-crate tower that stands perfectly still,
//! * a bin that 250 crates, barrels and balls rain into,
//! * a snake of dominoes that knocks itself over,
//! * ten ragdolls stumbling down a steep staircase together,
//! * a wrecking ball on a chain swinging into a crate wall,
//! * a seesaw catapult: a steel ball lands on one end and throws a crate,
//! * a motor-driven paddle wheel churning a bin of balls and blocks,
//! * a sagging plank bridge that crates drop onto, with a stand-in character on it,
//! * a kinematic turntable carrying crates and a stand-in character around,
//! * rubber balls bouncing in the middle,
//! * a pendulum wave: 15 bobs that fall out of step into waves and snakes and come back into
//!   step every minute, behind a Newton's cradle on a plinth passing its blow down the row,
//! * a pool table: the cue ball breaks a rack of fifteen and they scatter, bank off the cushions
//!   and drop into the pockets,
//! * a Galton board: five hundred steel balls pour through twelve rows of pins and pile up in
//!   seventeen bins in the bell curve printed behind them,
//! * a counterweight catapult: a tonne of stone swings its arm up into the crossbar and the
//!   granite ball flies on, 7 m up and 11 m down range into a crate wall,
//! * a giant Jenga tower: a test rig's rod pokes three loose blocks out, then one the tower
//!   rests on, and the top half comes down,
//! * along the front, three oil drums rolling down a ramp and 47 m along an asphalt runway
//!   (one triangle mesh) into a sand pit: they stay flat and in their lanes.
//!
//! The stations replay on their own cycles ([`Station::cycle_secs`]), so whoever walks in sees
//! them play; `physics.proving_ground.replay` restarts one or all of them now.

mod art;
pub mod catapult;
mod floaters;
pub mod galton;
pub mod jenga;
pub mod pool;

pub(crate) use floaters::replay_floaters;
pub use floaters::{FLOAT_CYCLE_SECS, FloatTest, spawn_floaters};

use art::kit::MeshBuilder;
use art::{Mat, Parts, dummy, props, structures};
use engine_assets::Assets;
use engine_core::math::{DVec3, Mat4, Quat, Vec2, Vec3};
use engine_core::prelude::*;
use engine_core::time::FixedTime;
use engine_render::Mesh;
use engine_render::prelude::MeshRenderer;
use engine_world::physics::{
    CharacterController, CharacterMove, Collider, ColliderShape, Joint, JointKind, JointMotor,
    RigidBody, Velocity,
};
use engine_world::{FocusPoints, SurfaceId, SurfaceMaterial, SurfaceMaterials};

/// Size of the concrete yard (x, z), meters.
pub const YARD_SIZE: [f64; 2] = [70.0, 50.0];

/// How far the yard's top stands above the ground it is poured on (m). A slab flush with the
/// ground fights it for depth, and the planet terrain's grass shows through in streaks.
pub const SLAB_RISE: f64 = 0.1;

/// Build the proving ground on the ground at `origin` (the center of the yard's footprint);
/// its top surface is [`SLAB_RISE`] above it.
pub fn spawn_demo(commands: &mut Commands, origin: DVec3) {
    let top = origin + DVec3::Y * SLAB_RISE;
    commands.queue(move |world: &mut World| build(world, top));
}

/// A suggested camera stop (showcase tour, screenshots): eye and target relative to the
/// yard's origin. Arriving restarts `station` (if any); `warmup_secs` later the action is on.
pub struct DemoView {
    pub key: &'static str,
    pub name: &'static str,
    pub eye: [f64; 3],
    pub target: [f64; 3],
    pub hour: f32,
    pub station: Option<Station>,
    pub warmup_secs: f32,
}

/// The proving ground's tour stops, one per action station.
pub const VIEWS: &[DemoView] = &[
    DemoView {
        key: "physics_ragdolls",
        name: "Ten crash test dummies tumbling down a staircase together",
        eye: [33.0, 4.5, -4.0],
        target: [24.0, 1.3, -12.0],
        hour: 16.0,
        station: Some(Station::Ragdolls),
        warmup_secs: 1.0,
    },
    DemoView {
        key: "physics_wrecking_ball",
        name: "A wrecking ball on a chain through a crate wall",
        eye: [-23.0, 4.0, 27.0],
        target: [-24.5, 2.5, 10.0],
        hour: 16.0,
        station: Some(Station::WreckingBall),
        warmup_secs: 1.9,
    },
    DemoView {
        key: "physics_seesaw",
        name: "Seesaw catapult and motor-driven paddle wheel",
        eye: [-1.0, 6.0, 25.0],
        target: [-4.0, 2.5, 9.0],
        hour: 16.0,
        station: Some(Station::Seesaw),
        // The ball lands at 1.25 s and throws the crate 13 m up; the tour's capture (warm-up
        // plus 0.4 s of exposure settling) catches it about 5 m up, still in frame.
        warmup_secs: 1.1,
    },
    DemoView {
        key: "physics_pile",
        name: "250 crates, drums and balls raining into a bin, by a 30-crate tower",
        eye: [-8.0, 9.0, 4.0],
        target: [-18.0, 3.0, -15.0],
        hour: 16.0,
        station: Some(Station::Pile),
        warmup_secs: 2.6,
    },
    DemoView {
        key: "physics_dominoes",
        name: "A domino run knocking itself over",
        eye: [8.0, 4.5, -4.0],
        target: [5.0, 0.4, -16.0],
        hour: 16.0,
        station: Some(Station::Dominoes),
        warmup_secs: 5.0,
    },
    DemoView {
        key: "physics_bridge",
        name: "Crates dropping onto a sagging plank bridge",
        eye: [27.5, 4.0, 10.0],
        target: [18.0, 2.6, 10.0],
        hour: 16.0,
        station: Some(Station::Bridge),
        warmup_secs: 6.0,
    },
    DemoView {
        key: "physics_barrel_run",
        name: "Oil drums rolling flat and straight down a 47 m runway into a sand pit",
        eye: [0.0, 1.7, 23.2],
        target: [-9.0, 0.35, 18.9],
        hour: 16.0,
        station: Some(Station::BarrelRun),
        warmup_secs: 7.5,
    },
    DemoView {
        key: "physics_rockfall",
        name: "300 granite rocks pouring from a hopper down a steel chute into a heap",
        eye: [-15.0, 4.0, 7.0],
        target: [-24.5, 2.0, -2.0],
        hour: 16.0,
        station: Some(Station::Rockfall),
        warmup_secs: 2.4,
    },
    DemoView {
        key: "physics_pendulum_wave",
        name: "A pendulum wave of 15 bobs falling into snakes, back in step every minute",
        eye: [16.0, 6.2, -3.4],
        target: [16.0, 1.5, -8.6],
        hour: 16.0,
        station: Some(Station::Pendulums),
        warmup_secs: 7.5,
    },
    DemoView {
        key: "physics_cradle",
        name: "A Newton's cradle passing the blow down its row of steel balls",
        eye: [17.4, 2.4, -2.2],
        target: [16.0, 1.75, -4.5],
        hour: 16.0,
        station: Some(Station::Cradle),
        warmup_secs: 0.9,
    },
    DemoView {
        key: "physics_pool",
        name: "A break on a pool table: fifteen balls scattering off the cushions into the pockets",
        eye: [8.6, 1.9, -6.6],
        target: [9.6, 0.8, -8.6],
        hour: 16.0,
        station: Some(Station::Pool),
        warmup_secs: 0.5,
    },
    DemoView {
        key: "physics_galton",
        name: "A Galton board: five hundred steel balls falling through pins into a bell curve",
        eye: [10.35, 1.3, -15.85],
        target: [10.0, 1.22, -18.0],
        hour: 16.0,
        station: Some(Station::Galton),
        warmup_secs: 6.0,
    },
    DemoView {
        key: "physics_catapult",
        name: "A counterweight catapult throwing a granite ball through a crate wall",
        eye: [0.0, 7.0, -11.0],
        target: [2.0, 4.0, 3.0],
        hour: 16.0,
        station: Some(Station::Catapult),
        // The ball leaves the cup at 1.0 s and tops its flight over the yard at 1.7 s.
        warmup_secs: 1.3,
    },
    DemoView {
        key: "physics_jenga",
        name: "A giant Jenga tower toppling when a test rig pushes out a block it rests on",
        eye: [-6.6, 2.3, -1.4],
        target: [-10.0, 1.35, -5.5],
        hour: 16.0,
        station: Some(Station::Jenga),
        // Three loose blocks are out by 17 s; the top half is tipping over at 22.4 s.
        warmup_secs: 22.4,
    },
    DemoView {
        key: "physics_turntable",
        name: "A turntable carrying crates and a rider",
        eye: [37.0, 6.5, 20.0],
        target: [30.0, 0.5, 10.0],
        hour: 16.0,
        station: None,
        warmup_secs: 0.0,
    },
];

/// The proving ground's stations, back row to front row.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Station {
    Tower,
    Pile,
    Dominoes,
    Ragdolls,
    WreckingBall,
    Seesaw,
    PaddleWheel,
    Bridge,
    Turntable,
    Balls,
    BarrelRun,
    Rockfall,
    Pendulums,
    Cradle,
    Pool,
    Galton,
    Catapult,
    Jenga,
}

impl Station {
    pub const ALL: [Station; 18] = [
        Station::Tower,
        Station::Pile,
        Station::Dominoes,
        Station::Ragdolls,
        Station::WreckingBall,
        Station::Seesaw,
        Station::PaddleWheel,
        Station::Bridge,
        Station::Turntable,
        Station::Balls,
        Station::BarrelRun,
        Station::Rockfall,
        Station::Pendulums,
        Station::Cradle,
        Station::Pool,
        Station::Galton,
        Station::Catapult,
        Station::Jenga,
    ];

    /// The station's name in the `physics.proving_ground.replay` API.
    pub fn key(self) -> &'static str {
        match self {
            Station::Tower => "tower",
            Station::Pile => "pile",
            Station::Dominoes => "dominoes",
            Station::Ragdolls => "ragdolls",
            Station::WreckingBall => "wrecking_ball",
            Station::Seesaw => "seesaw",
            Station::PaddleWheel => "paddle_wheel",
            Station::Bridge => "bridge",
            Station::Turntable => "turntable",
            Station::Balls => "balls",
            Station::BarrelRun => "barrel_run",
            Station::Rockfall => "rockfall",
            Station::Pendulums => "pendulums",
            Station::Cradle => "cradle",
            Station::Pool => "pool",
            Station::Galton => "galton",
            Station::Catapult => "catapult",
            Station::Jenga => "jenga",
        }
    }

    /// Where it stands in the yard (yard-local, meters).
    pub fn base(self) -> DVec3 {
        let (x, z) = match self {
            Station::Tower => (-28.0, -16.0),
            Station::Pile => (-14.0, -14.0),
            Station::Dominoes => (-3.0, -16.0),
            Station::Ragdolls => (24.0, -10.0),
            Station::WreckingBall => (-26.0, 10.0),
            Station::Seesaw => (-10.0, 10.0),
            Station::PaddleWheel => (4.0, 10.0),
            Station::Bridge => (18.0, 10.0),
            Station::Turntable => (30.0, 10.0),
            Station::Balls => (6.0, -3.0),
            Station::BarrelRun => (0.0, 19.5),
            Station::Rockfall => (-24.0, -2.0),
            Station::Pendulums => (16.0, -8.5),
            Station::Cradle => (16.0, -4.5),
            Station::Pool => (9.0, -8.5),
            Station::Galton => (10.0, -18.0),
            Station::Catapult => (-4.0, 3.0),
            Station::Jenga => (-10.0, -5.5),
        };
        DVec3::new(x, 0.0, z)
    }

    /// World seconds between replays: a little longer than the station takes to play out and
    /// settle. `None` for the ones that never stop (the tower stands, the wheel and table turn).
    pub fn cycle_secs(self) -> Option<f64> {
        match self {
            Station::Tower | Station::PaddleWheel | Station::Turntable => None,
            Station::Pile => Some(45.0),
            Station::Dominoes | Station::WreckingBall | Station::Bridge => Some(30.0),
            Station::Ragdolls | Station::Seesaw | Station::Balls => Some(20.0),
            Station::BarrelRun => Some(25.0),
            Station::Rockfall => Some(30.0),
            // The wave is back in step after exactly its cycle, so the replay is seamless.
            Station::Pendulums => Some(WAVE_CYCLE_SECS as f64),
            Station::Cradle => Some(20.0),
            Station::Pool => Some(25.0),
            Station::Galton => Some(40.0),
            Station::Catapult => Some(20.0),
            // The last poke starts at 18 s; the top half is down and still by 30.
            Station::Jenga => Some(32.0),
        }
    }

    pub fn from_key(key: &str) -> Option<Station> {
        Station::ALL.into_iter().find(|s| s.key() == key)
    }

    fn build(self, y: &mut Yard) {
        let base = self.base();
        match self {
            Station::Tower => tower(y, base),
            Station::Pile => pile(y, base),
            Station::Dominoes => dominoes(y, base),
            Station::Ragdolls => ragdoll_stairs(y, base),
            Station::WreckingBall => wrecking_ball(y, base),
            Station::Seesaw => seesaw(y, base),
            Station::PaddleWheel => paddle_wheel(y, base),
            Station::Bridge => bridge(y, base),
            Station::Turntable => turntable(y, base),
            Station::Balls => bouncy_balls(y, base),
            Station::BarrelRun => barrel_run(y, base),
            Station::Rockfall => rockfall(y, base),
            Station::Pendulums => pendulum_wave(y, base),
            Station::Cradle => newtons_cradle(y, base),
            Station::Pool => pool::pool_table(y, base),
            Station::Galton => galton::galton_board(y, base),
            Station::Catapult => catapult::catapult(y, base),
            Station::Jenga => jenga::jenga(y, base),
        }
    }
}

/// A proving ground, on its yard's pad entity: where it is, its surfaces, and how long each
/// station has run since it was last (re)built.
#[derive(Component, Debug, Clone)]
pub struct ProvingGround {
    origin: DVec3,
    surfaces: [SurfaceId; 4],
    running: [f64; Station::ALL.len()],
    /// Replay stations on their cycles (on by default).
    pub replay: bool,
}

/// Marks everything a station spawned, so a replay clears it.
#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct StationPart {
    ground: Entity,
    station: Station,
}

struct Yard<'w> {
    world: &'w mut World,
    origin: DVec3,
    /// Tags what the yard spawns (the station being built).
    tag: Option<StationPart>,
    /// Everything spawned so far, children included.
    spawned: Vec<Entity>,
    /// The app draws meshes (there is an `Assets<Mesh>`); headless physics apps get colliders
    /// only.
    render: bool,
    concrete: SurfaceId,
    wood: SurfaceId,
    metal: SurfaceId,
    rubber: SurfaceId,
}

impl Yard<'_> {
    fn at(&self, local: DVec3) -> DVec3 {
        self.origin + local
    }

    fn spawn(&mut self, bundle: impl Bundle) -> Entity {
        let mut e = self.world.spawn(bundle);
        if let Some(tag) = self.tag {
            e.insert(tag);
        }
        let e = e.id();
        self.spawned.push(e);
        e
    }

    /// `N` meshes cached under `key` (built by `make` the first time), each with its material.
    fn parts<const N: usize>(
        &mut self,
        key: &str,
        mats: [Mat; N],
        make: impl FnOnce() -> [Mesh; N],
    ) -> Parts {
        if !self.render {
            return Parts::new();
        }
        art::parts(self.world, key, mats, make)
    }

    /// Draw `parts` on `e` (as children when there are several materials).
    fn dress(&mut self, e: Entity, parts: Parts) {
        if let [(mesh, material)] = parts[..] {
            self.world
                .entity_mut(e)
                .insert(MeshRenderer::new(mesh, material));
            return;
        }
        for (mesh, material) in parts {
            self.spawn((
                Transform::default(),
                MeshRenderer::new(mesh, material),
                ChildOf(e),
            ));
        }
    }

    /// Render-only scenery (the colliders under it are separate boxes).
    fn scenery(&mut self, local: DVec3, rotation: Quat, parts: Parts) {
        let t = Transform::from_translation(self.at(local)).with_rotation(rotation);
        for (mesh, material) in parts {
            self.spawn((t, MeshRenderer::new(mesh, material)));
        }
    }

    /// A static concrete box, drawn by the scenery around it.
    fn block(&mut self, local: DVec3, half: Vec3) -> Entity {
        let at = self.at(local);
        self.spawn((
            Transform::from_translation(at),
            RigidBody::fixed(),
            Collider::cuboid(half).with_surface(self.concrete),
        ))
    }

    fn body(&mut self, local: DVec3, rotation: Quat, collider: Collider, parts: Parts) -> Entity {
        let at = self.at(local);
        let e = self.spawn((
            Transform::from_translation(at).with_rotation(rotation),
            RigidBody::dynamic(),
            collider,
            Velocity::default(),
        ));
        self.dress(e, parts);
        e
    }

    /// A wooden shipping crate (density 300 kg/m³: a crate with stuff in it). `seed` varies
    /// its boards.
    fn crate_box(&mut self, local: DVec3, half: Vec3, rotation: Quat, seed: u32) -> Entity {
        self.crate_of(local, half, rotation, seed, 300.0)
    }

    /// A wooden crate of `density` kg/m³.
    fn crate_of(
        &mut self,
        local: DVec3,
        half: Vec3,
        rotation: Quat,
        seed: u32,
        density: f32,
    ) -> Entity {
        let c = Collider::cuboid(half)
            .with_surface(self.wood)
            .with_density(density);
        let key = format!("crate/{:.3}x{:.3}x{:.3}/{seed}", half.x, half.y, half.z);
        let parts = self.parts(&key, [Mat::CrateWood(seed as u8), Mat::Dark], || {
            let m = props::crate_box(half, seed);
            [m.wood, m.metal]
        });
        self.body(local, rotation, c, parts)
    }

    /// A rubber playground ball of `color` (sRGB hex).
    fn ball(&mut self, local: DVec3, radius: f32, density: f32, color: u32) -> Entity {
        let c = Collider::sphere(radius)
            .with_surface(self.rubber)
            .with_density(density);
        let parts = self.parts(&format!("ball/{radius:.3}"), [Mat::Rubber(color)], || {
            [props::playground_ball(radius)]
        });
        self.body(local, Quat::IDENTITY, c, parts)
    }

    /// A solid steel ball (7.8 t/m³).
    fn steel_ball(&mut self, local: DVec3, radius: f32, parts: Parts) -> Entity {
        let c = Collider::sphere(radius)
            .with_surface(self.metal)
            .with_density(7800.0);
        self.body(local, Quat::IDENTITY, c, parts)
    }

    fn capsule(
        &mut self,
        local: DVec3,
        rotation: Quat,
        radius: f32,
        half_height: f32,
        parts: Parts,
    ) -> Entity {
        let c = Collider::capsule(radius, half_height).with_density(1000.0);
        self.body(local, rotation, c, parts)
    }

    /// A joint at world point `at` (yard-local), with anchors computed from the bodies' poses.
    fn joint(&mut self, kind: JointKind, a: Entity, b: Option<Entity>, local: DVec3) -> Entity {
        let at = self.at(local);
        let ta = *self.world.get::<Transform>(a).unwrap();
        let anchor_a = ta.rotation.inverse() * (at - ta.translation).as_vec3();
        let anchor_b = match b {
            Some(b) => {
                let tb = *self.world.get::<Transform>(b).unwrap();
                (tb.rotation.inverse() * (at - tb.translation).as_vec3()).as_dvec3()
            }
            None => at,
        };
        self.spawn(Joint {
            kind,
            body_a: a,
            body_b: b,
            anchor_a,
            anchor_b,
            break_force: None,
            collide_connected: false,
        })
    }

    /// A stand-in character with its feet at `feet`: the controller the movement module
    /// drives, here only falling at a steady 2 m/s, so it stands, rides and weighs. Drawn as a
    /// crash test dummy whose jacket is `shirt` (sRGB hex).
    fn mannequin(&mut self, feet: DVec3, shirt: u32, name: &str) -> Entity {
        let desc = CharacterController::default();
        let center = feet + DVec3::Y * (desc.height as f64 * 0.5 + 0.02);
        let e = self.spawn((
            Transform::from_translation(self.at(center)),
            desc,
            CharacterMove {
                velocity: Vec3::new(0.0, -2.0, 0.0),
            },
            Name::new(name.to_string()),
        ));
        let height = desc.height;
        let parts = self.parts(
            &format!("dummy/standing/{height:.2}/{shirt:06x}"),
            [Mat::Vinyl, Mat::Steel],
            || {
                let d = dummy::standing_dummy(height, art::kit::srgb(shirt));
                [d.vinyl, d.steel]
            },
        );
        self.dress(e, parts);
        e
    }

    /// A static collider of `shape` on `surface`.
    fn fixed(
        &mut self,
        local: DVec3,
        rotation: Quat,
        shape: ColliderShape,
        surface: SurfaceId,
    ) -> Entity {
        let at = self.at(local);
        self.spawn((
            Transform::from_translation(at).with_rotation(rotation),
            RigidBody::fixed(),
            Collider {
                shape,
                surface,
                ..Default::default()
            },
        ))
    }

    fn name(&mut self, e: Entity, name: &str) {
        self.world.entity_mut(e).insert(Name::new(name.to_string()));
    }
}

/// The demo's surfaces (concrete, wood, metal, rubber), adding rubber to the table if needed.
fn surfaces(world: &mut World) -> [SurfaceId; 4] {
    let mut table = world.get_resource_or_init::<SurfaceMaterials>();
    let rubber = match table.id("rubber") {
        Some(id) => id,
        None => table.add(SurfaceMaterial {
            name: "rubber".into(),
            friction: 0.9,
            restitution: 0.85,
            density: 1100.0,
            footstep_sound: "footstep.rubber".into(),
            impact_effect: "impact.rubber".into(),
            ..Default::default()
        }),
    };
    [
        table.id_or_default("concrete"),
        table.id_or_default("wood"),
        table.id_or_default("metal"),
        rubber,
    ]
}

fn build(world: &mut World, origin: DVec3) {
    let surfaces = surfaces(world);
    let mut y = Yard::new(world, origin, surfaces, None);
    let half = Vec3::new(YARD_SIZE[0] as f32 * 0.5, 0.25, YARD_SIZE[1] as f32 * 0.5);
    let pad = y.block(DVec3::new(0.0, -0.25, 0.0), half);
    y.name(pad, "Proving ground");
    // A curb around the yard keeps rolling crates and balls in.
    let (hx, hz, curb_h, curb_t) = (half.x, half.z, 0.8f32, 0.4f32);
    for s in [-1.0f32, 1.0] {
        y.block(
            DVec3::new(0.0, curb_h as f64 * 0.5, (s * (hz - curb_t * 0.5)) as f64),
            Vec3::new(hx, curb_h * 0.5, curb_t * 0.5),
        );
        y.block(
            DVec3::new((s * (hx - curb_t * 0.5)) as f64, curb_h as f64 * 0.5, 0.0),
            Vec3::new(curb_t * 0.5, curb_h * 0.5, hz - curb_t),
        );
    }
    let size = YARD_SIZE.map(|v| v as f32);
    let slab = y.parts("yard", [Mat::Slab, Mat::Concrete, Mat::Hazard], || {
        let m = structures::yard(size, 0.5, curb_h, curb_t);
        [m.slab, m.curb, m.hazard]
    });
    y.scenery(DVec3::ZERO, Quat::IDENTITY, slab);
    site(&mut y, [hx - curb_t, hz - curb_t]);
    world.entity_mut(pad).insert(ProvingGround {
        origin,
        surfaces,
        running: [0.0; Station::ALL.len()],
        replay: true,
    });
    for station in Station::ALL {
        build_station(world, pad, station);
    }
}

/// Where the site office stands (yard-local, its floor's middle): on the yard's east side
/// between the stairs and the turntable, long along Z, its door facing the stations (-X).
const OFFICE_AT: DVec3 = DVec3::new(31.0, 0.0, -4.0);

/// What makes the yard a working test site: the painted walkway inside the curb (`inner`, its
/// inner face's half extents), a floodlight mast at each corner and the site office.
fn site(y: &mut Yard, inner: [f32; 2]) {
    let paint = y.parts("yard_walkway", [Mat::LinePaint], || {
        [art::site::walkway(inner)]
    });
    y.scenery(DVec3::ZERO, Quat::IDENTITY, paint);
    let mast = y.parts(
        "yard_mast",
        [Mat::Galvanized, Mat::Dark, Mat::Glass],
        || {
            let m = art::site::mast();
            [m.steel, m.dark, m.glass]
        },
    );
    let high = art::site::MAST_HIGH;
    for (sx, sz) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
        let foot = Vec3::new(sx * (inner[0] - 0.35), 0.0, sz * (inner[1] - 0.35)).as_dvec3();
        y.fixed(
            foot + DVec3::Y * (high * 0.5) as f64,
            Quat::IDENTITY,
            ColliderShape::Cylinder {
                radius: art::site::MAST_FOOT,
                half_height: high * 0.5,
            },
            y.metal,
        );
        // The lamps (+Z in the mast's frame) look in toward the yard's middle.
        y.scenery(foot, Quat::from_rotation_y((-sx).atan2(-sz)), mast.clone());
    }
    let office = y.parts(
        "yard_office",
        [Mat::Drum(0x3d5a73), Mat::Dark, Mat::Glass],
        || {
            let o = art::site::office();
            [o.paint, o.dark, o.glass]
        },
    );
    let half = art::site::OFFICE;
    let facing = Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2);
    y.fixed(
        OFFICE_AT + DVec3::Y * half.y as f64,
        facing,
        ColliderShape::Cuboid { half_extents: half },
        y.metal,
    );
    y.scenery(OFFICE_AT, facing, office);
}

impl<'w> Yard<'w> {
    fn new(
        world: &'w mut World,
        origin: DVec3,
        [concrete, wood, metal, rubber]: [SurfaceId; 4],
        tag: Option<StationPart>,
    ) -> Self {
        let render = world.contains_resource::<Assets<Mesh>>();
        Yard {
            world,
            origin,
            tag,
            spawned: Vec::new(),
            render,
            concrete,
            wood,
            metal,
            rubber,
        }
    }
}

/// (Re)build `station` of the proving ground on `ground`: clear what it spawned last time and
/// spawn it fresh.
fn build_station(world: &mut World, ground: Entity, station: Station) {
    let Some(g) = world.get::<ProvingGround>(ground).cloned() else {
        return;
    };
    let old: Vec<Entity> = world
        .query::<(Entity, &StationPart)>()
        .iter(world)
        .filter(|(_, p)| p.ground == ground && p.station == station)
        .map(|(e, _)| e)
        .collect();
    for e in old {
        // Children go with their parent, so some are gone already.
        if let Ok(e) = world.get_entity_mut(e) {
            e.despawn();
        }
    }
    let tag = StationPart { ground, station };
    station.build(&mut Yard::new(world, g.origin, g.surfaces, Some(tag)));
    if let Some(mut g) = world.get_mut::<ProvingGround>(ground) {
        g.running[station as usize] = 0.0;
    }
}

/// Proving grounds farther than this from every focus point pause their replays (nobody sees
/// them, and far bodies are frozen anyway), meters.
const REPLAY_RANGE_M: f64 = 250.0;

/// Replay each proving ground's stations whose cycle is up (world time, so slow motion slows
/// the cycles too).
pub(crate) fn replay_stations(world: &mut World) {
    let dt = world
        .get_resource::<FixedTime>()
        .map_or(0.0, |t| t.world_step_secs());
    let focus = world
        .get_resource::<FocusPoints>()
        .map(|f| f.points.clone())
        .unwrap_or_default();
    let watched =
        |at: DVec3| focus.is_empty() || focus.iter().any(|p| p.distance(at) < REPLAY_RANGE_M);
    let mut due = Vec::new();
    for (e, mut g) in world
        .query::<(Entity, &mut ProvingGround)>()
        .iter_mut(world)
    {
        if !g.replay || !watched(g.origin) {
            continue;
        }
        for station in Station::ALL {
            let t = &mut g.running[station as usize];
            *t += dt;
            if station.cycle_secs().is_some_and(|c| *t >= c) {
                due.push((e, station));
            }
        }
    }
    for (e, station) in due {
        build_station(world, e, station);
    }
}

/// `physics.proving_ground.replay`: restart one station (`{"station": "dominoes"}`) or all of
/// them (`{}`) on every proving ground. Returns the stations restarted.
pub(crate) fn replay_api(
    world: &mut World,
    args: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let stations: Vec<Station> = match args.get("station").and_then(|v| v.as_str()) {
        Some(key) => vec![Station::from_key(key).ok_or_else(|| {
            let keys: Vec<_> = Station::ALL.iter().map(|s| s.key()).collect();
            format!("unknown station `{key}`; stations: {}", keys.join(", "))
        })?],
        None => Station::ALL.to_vec(),
    };
    let grounds: Vec<Entity> = world
        .query_filtered::<Entity, With<ProvingGround>>()
        .iter(world)
        .collect();
    for &g in &grounds {
        for &s in &stations {
            build_station(world, g, s);
        }
    }
    Ok(serde_json::json!({
        "proving_grounds": grounds.len(),
        "stations": stations.iter().map(|s| s.key()).collect::<Vec<_>>(),
    }))
}

/// Deterministic pseudo-random numbers in 0..1 (the demo looks the same every run).
struct Rng(u32);

impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 >> 8) as f32 / (1u32 << 24) as f32
    }
}

/// Board patterns per crate size: crates of one size share their meshes.
const CRATE_SEEDS: u32 = 3;

fn tower(y: &mut Yard, base: DVec3) {
    let half = Vec3::splat(0.3);
    for i in 0..30 {
        let e = y.crate_box(
            base + DVec3::new(0.0, 0.3 + 0.6 * i as f64, 0.0),
            half,
            // Every other crate turned a quarter: the boards don't line up like wallpaper.
            Quat::from_rotation_y((i % 2) as f32 * std::f32::consts::FRAC_PI_2),
            i as u32 % CRATE_SEEDS,
        );
        if i == 29 {
            y.name(e, "Tower top crate");
        }
    }
}

/// Crate sizes in the pile (half extents, meters).
const PILE_CRATES: [Vec3; 5] = [
    Vec3::new(0.3, 0.25, 0.3),
    Vec3::new(0.42, 0.3, 0.32),
    Vec3::new(0.26, 0.22, 0.26),
    Vec3::new(0.36, 0.33, 0.38),
    Vec3::new(0.45, 0.2, 0.28),
];

/// Oil drum colors (sRGB).
const DRUM_COLORS: [u32; 3] = [0x2f5f9e, 0xb8392f, 0x3c7d3a];

/// A bin (8 × 8 m inside) and 250 crates, barrels and balls dropped into it.
fn pile(y: &mut Yard, center: DVec3) {
    for (dx, dz, hx, hz) in [
        (4.15, 0.0, 0.15, 4.3),
        (-4.15, 0.0, 0.15, 4.3),
        (0.0, 4.15, 4.0, 0.15),
        (0.0, -4.15, 4.0, 0.15),
    ] {
        y.block(center + DVec3::new(dx, 0.8, dz), Vec3::new(hx, 0.8, hz));
    }
    let bin = y.parts("pile_bin", [Mat::Concrete, Mat::Yellow, Mat::Steel], || {
        let b = structures::pile_bin(Vec2::new(4.0, 4.0), 0.3, 1.6);
        [b.concrete, b.painted, b.steel]
    });
    y.scenery(center, Quat::IDENTITY, bin);
    let mut rng = Rng(0x5eed_1234);
    for i in 0..250 {
        let (gx, gz, layer) = ((i % 6) as f64, ((i / 6) % 6) as f64, (i / 36) as f64);
        let at = center
            + DVec3::new(
                (gx - 2.5) * 1.2 + rng.next() as f64 * 0.2,
                2.5 + layer * 1.3,
                (gz - 2.5) * 1.2 + rng.next() as f64 * 0.2,
            );
        let rotation = Quat::from_euler(
            EulerRot::XYZ,
            rng.next() * TAU,
            rng.next() * TAU,
            rng.next() * TAU,
        );
        match i % 5 {
            0..=2 => {
                let half = PILE_CRATES[(rng.next() * PILE_CRATES.len() as f32) as usize];
                y.crate_box(at, half, rotation, i as u32 % CRATE_SEEDS);
            }
            3 => {
                // An oil barrel.
                let c = Collider {
                    shape: ColliderShape::Cylinder {
                        radius: 0.3,
                        half_height: 0.42,
                    },
                    surface: y.metal,
                    density: 400.0,
                    ..Default::default()
                };
                let color = DRUM_COLORS[i % 3];
                let parts = y.parts(&format!("drum/{color:06x}"), [Mat::Drum(color)], || {
                    [props::steel_drum(0.3, 0.42)]
                });
                y.body(at, rotation, c, parts);
            }
            _ => {
                y.ball(at, 0.3, 300.0, 0xd8c23a);
            }
        }
    }
}

/// Dominoes along an S curve; the first one is already tipping.
fn dominoes(y: &mut Yard, start: DVec3) {
    let curve = |t: f64| {
        DVec3::new(
            start.x + 17.0 * t,
            0.0,
            start.z + 4.0 * (t * 3.0 * std::f64::consts::PI).sin(),
        )
    };
    // Walk the curve in fine steps and drop a domino every 0.5 m.
    let mut placed = Vec::new();
    let (mut last, mut acc) = (curve(0.0), 0.5);
    for k in 1..=4000 {
        let p = curve(k as f64 / 4000.0);
        acc += p.distance(last);
        if acc >= 0.5 {
            let dir = (p - last).normalize();
            placed.push((p, dir));
            acc = 0.0;
        }
        last = p;
    }
    let half = Vec3::new(0.25, 0.5, 0.06);
    for (i, (p, dir)) in placed.iter().enumerate() {
        let d = dir.as_vec3();
        // The domino's thin side (local z) faces along the path.
        let rotation = Quat::from_rotation_arc(Vec3::Z, d);
        let c = Collider::cuboid(half)
            .with_surface(y.wood)
            .with_density(700.0);
        // Ivory and black in turn, the pips walking through a double-six set.
        let (ivory, pips) = (i % 2 == 0, ((i % 7) as u8, ((i / 7) % 7) as u8));
        let parts = y.parts(
            &format!("domino/{}{}{}", pips.0, pips.1, ivory as u8),
            [Mat::Domino],
            || [props::domino(half, pips, ivory)],
        );
        let e = y.body(*p + DVec3::Y * 0.5, rotation, c, parts);
        if i == 0 {
            y.name(e, "First domino");
            // Tip it forward: spin about the horizontal axis across the path.
            y.world.get_mut::<Velocity>(e).unwrap().angular = Vec3::Y.cross(d) * 2.5;
        }
    }
}

/// A 10-part crash test dummy ragdoll (Hybrid III 50th-percentile proportions) facing +z with
/// feet at `feet`, stumbling along `push` (horizontal, m/s). Returns its chest.
fn ragdoll(y: &mut Yard, feet: DVec3, shirt: u32, push: Vec3) -> Entity {
    let l = dummy::RagdollLayout::HYBRID_III_50TH;
    // Each part's vinyl and steel meshes, in `RagdollMeshes::all` order.
    let meshes = y.parts::<22>(
        &format!("dummy/ragdoll/{shirt:06x}"),
        [[Mat::Vinyl, Mat::Steel]; 11].concat().try_into().unwrap(),
        || {
            let m = dummy::ragdoll_parts(&l, art::kit::srgb(shirt));
            let all: Vec<Mesh> = m
                .all()
                .into_iter()
                .flat_map(|p| [p.vinyl.clone(), p.steel.clone()])
                .collect();
            all.try_into().unwrap()
        },
    );
    let mut part = meshes.chunks(2).map(|c| c.to_vec());
    // `meshes` is empty without a renderer.
    let mut next = || part.next().unwrap_or_default();
    let at = |p: Vec3| feet + p.as_dvec3();
    let pelvis = {
        let c = Collider::cuboid(l.pelvis_half).with_density(1000.0);
        y.body(at(l.pelvis), Quat::IDENTITY, c, next())
    };
    let chest = y.capsule(
        at(l.chest.center),
        Quat::IDENTITY,
        l.chest.radius,
        l.chest.half_height,
        next(),
    );
    let head = {
        let c = Collider::sphere(l.head_radius).with_density(1000.0);
        y.body(at(l.head), Quat::IDENTITY, c, next())
    };
    let mut parts = vec![pelvis, chest, head];
    let cone = |c: f32| JointKind::Spherical {
        cone_limit: Some(c),
    };
    // Hinges about X: elbows bend the hand forward (negative angles), knees the foot back.
    let hinge = |limits: [f32; 2]| JointKind::Revolute {
        axis: Vec3::X,
        limits: Some(limits),
        motor: None,
    };
    y.joint(cone(0.5), pelvis, Some(chest), at(l.waist));
    y.joint(cone(0.6), chest, Some(head), at(l.neck));
    // Meshes come right then left for each limb.
    let limb_meshes: Vec<[Parts; 2]> = (0..4).map(|_| [next(), next()]).collect();
    for (k, side) in [-1.0f32, 1.0].into_iter().enumerate() {
        let m = |p: Vec3| at(Vec3::new(side * p.x, p.y, p.z));
        let limb = |y: &mut Yard, spec: dummy::CapsuleSpec, parts: Parts| {
            y.capsule(
                m(spec.center),
                Quat::IDENTITY,
                spec.radius,
                spec.half_height,
                parts,
            )
        };
        let upper = limb(y, l.upper_arm, limb_meshes[0][k].clone());
        let fore = limb(y, l.forearm, limb_meshes[1][k].clone());
        let thigh = limb(y, l.thigh, limb_meshes[2][k].clone());
        let shin = limb(y, l.shin, limb_meshes[3][k].clone());
        y.joint(cone(1.4), chest, Some(upper), m(l.shoulder));
        y.joint(hinge([-2.2, 0.0]), upper, Some(fore), m(l.elbow));
        y.joint(cone(1.2), pelvis, Some(thigh), m(l.hip));
        y.joint(hinge([0.0, 2.2]), thigh, Some(shin), m(l.knee));
        parts.extend([upper, fore, thigh, shin]);
    }
    // Stumbling: the whole body moves, the upper body a little faster so it pitches.
    let lean = push.normalize_or_zero();
    for p in parts {
        let v = if p == chest || p == head {
            push + lean
        } else {
            push
        };
        y.world.get_mut::<Velocity>(p).unwrap().linear = v + Vec3::Y * 0.5;
    }
    chest
}

/// How fast the dummies stumble off the landing (m/s), give or take 15%.
const RAGDOLL_PUSH: f32 = 3.5;

/// The dummies on the stairs' landing: across (m from the middle), back from its front edge
/// (m), jacket (sRGB hex) and name. Four in front, three and three staggered behind them.
const DUMMIES: [(f64, f64, u32, &str); 10] = [
    (-1.8, 0.45, 0x3a6ea5, "blue"),
    (-0.6, 0.45, 0xb4413a, "red"),
    (0.6, 0.45, 0x4e8a3e, "green"),
    (1.8, 0.45, 0xd9822b, "orange"),
    (-1.2, 1.25, 0x6b4c9a, "purple"),
    (0.0, 1.25, 0x2a8c8c, "teal"),
    (1.2, 1.25, 0xd8d6cc, "white"),
    (-1.8, 2.05, 0x3b3d42, "charcoal"),
    (0.0, 2.05, 0x7a2e3b, "maroon"),
    (1.8, 2.05, 0x6b6b2a, "olive"),
];

/// Fourteen steep steps up toward -z with a landing, and ten ragdolls stumbling down them
/// together, coming to rest strewn down the flight and at its foot, and falling asleep.
fn ragdoll_stairs(y: &mut Yard, foot: DVec3) {
    let (rise, run, steps, width, deep) = (0.22, 0.32, 14, 6.0, 3.5);
    for i in 0..steps {
        let h = rise * (i + 1) as f64;
        y.block(
            foot + DVec3::new(0.0, h * 0.5, -run * (i as f64 + 0.5)),
            Vec3::new(width * 0.5, h as f32 * 0.5, run as f32 * 0.5),
        );
    }
    let top = rise * steps as f64;
    let landing = foot.z - run * steps as f64 - deep as f64 * 0.5;
    y.block(
        DVec3::new(foot.x, top * 0.5, landing),
        Vec3::new(width * 0.5, top as f32 * 0.5, deep * 0.5),
    );
    let flight = y.parts("stairs", [Mat::Concrete, Mat::Yellow], || {
        let s = structures::stairs(rise as f32, run as f32, steps, width, deep);
        [s.concrete, s.steel]
    });
    y.scenery(foot, Quat::IDENTITY, flight);
    let front = landing + deep as f64 * 0.5;
    let mut rng = Rng(0xd0_11ed);
    for (dx, back, shirt, color) in DUMMIES {
        // Each stumbles a little faster or slower, and a little to one side.
        let speed = RAGDOLL_PUSH * (0.85 + 0.3 * rng.next());
        let heading = (rng.next() - 0.5) * 0.25;
        let push = Vec3::new(heading.sin(), 0.0, heading.cos()) * speed;
        let feet = DVec3::new(foot.x + dx, top, front - back);
        let chest = ragdoll(y, feet, shirt, push);
        y.name(chest, &format!("Ragdoll ({color})"));
    }
}

/// A gantry with a steel ball on a 6-link chain, let go at 70°, and a crate wall in its path.
fn wrecking_ball(y: &mut Yard, base: DVec3) {
    let pivot = base + DVec3::new(0.0, 7.6, 0.0);
    for dz in [-2.4, 2.4] {
        y.block(base + DVec3::new(0.0, 3.9, dz), Vec3::new(0.25, 3.9, 0.25));
    }
    y.block(pivot + DVec3::Y * 0.3, Vec3::new(0.25, 0.25, 2.65));
    let gantry = y.parts("gantry", [Mat::Yellow, Mat::Steel, Mat::Concrete], || {
        let g = structures::gantry(7.6, 4.8);
        [g.painted, g.steel, g.concrete]
    });
    y.scenery(base, Quat::IDENTITY, gantry);

    let angle = 70f32.to_radians();
    // Pulled back toward -x (the yard's edge), so the wall flies into the yard.
    let down = Vec3::new(-angle.sin(), -angle.cos(), 0.0).as_dvec3();
    let rotation = Quat::from_rotation_z(-angle);
    let link = props::CHAIN_SEGMENT_LENGTH as f64;
    let (wire, links) = (props::CHAIN_WIRE, 6);
    let mut prev: Option<Entity> = None;
    for i in 0..links {
        let center = pivot + down * (link * (i as f64 + 0.5));
        // The last segment's bottom links are the ball's shackle.
        let end = i == links - 1;
        let chain = y.parts(&format!("chain/{end}"), [Mat::Steel], || {
            [if end {
                props::chain_end_segment(link as f32, wire)
            } else {
                props::chain_segment(link as f32, wire)
            }]
        });
        let e = y.capsule(center, rotation, 0.07, 0.33, chain);
        let top = pivot + down * (link * i as f64);
        let kind = JointKind::Spherical { cone_limit: None };
        match prev {
            None => y.joint(kind, e, None, top),
            Some(p) => y.joint(kind, p, Some(e), top),
        };
        prev = Some(e);
    }
    let radius = 0.8;
    let center = pivot + down * (link * links as f64 + radius as f64);
    let parts = y.parts("wrecking_ball", [Mat::Steel], || {
        [props::wrecking_ball(radius)]
    });
    let ball = y.steel_ball(center, radius, parts);
    y.world.get_mut::<Transform>(ball).unwrap().rotation = rotation;
    y.name(ball, "Wrecking ball");
    y.joint(
        JointKind::Spherical { cone_limit: None },
        prev.unwrap(),
        Some(ball),
        pivot + down * (link * links as f64),
    );

    // The wall: 6 × 4 crates on the far side of the swing.
    let half = Vec3::splat(0.4);
    for row in 0..4 {
        for col in 0..6 {
            y.crate_box(
                base + DVec3::new(1.8, 0.4 + 0.8 * row as f64, -2.0 + 0.8 * col as f64),
                half,
                Quat::IDENTITY,
                (row + col) as u32 % CRATE_SEEDS,
            );
        }
    }
}

/// A plank on a hinge; a 7 t steel ball lands on one end and throws the crate on the other.
fn seesaw(y: &mut Yard, base: DVec3) {
    let stand_half = Vec3::new(0.3, 0.4, 0.6);
    let stand = y.block(base + DVec3::new(0.0, 0.4, 0.0), stand_half);
    let parts = y.parts("seesaw_stand", [Mat::Dark, Mat::Steel], || {
        let s = structures::seesaw_stand(stand_half);
        [s.painted, s.steel]
    });
    y.dress(stand, parts);
    let half = Vec3::new(4.0, 0.1, 0.6);
    let plank_c = Collider::cuboid(half)
        .with_surface(y.wood)
        .with_density(300.0);
    let parts = y.parts(
        "seesaw_plank",
        [Mat::Timber, Mat::Dark, Mat::Steel, Mat::Rubber(0x1e1e20)],
        || {
            let p = structures::seesaw_plank(half);
            [p.timber, p.painted, p.steel, p.rubber]
        },
    );
    let plank = y.body(
        base + DVec3::new(0.0, 0.9, 0.0),
        Quat::IDENTITY,
        plank_c,
        parts,
    );
    y.name(plank, "Seesaw");
    y.joint(
        JointKind::Revolute {
            axis: Vec3::Z,
            limits: Some([-0.35, 0.35]),
            motor: None,
        },
        plank,
        None,
        base + DVec3::new(0.0, 0.8, 0.0),
    );
    let c = y.crate_box(
        base + DVec3::new(-3.5, 1.4, 0.0),
        Vec3::splat(0.4),
        Quat::IDENTITY,
        1,
    );
    if let Some(mut col) = y.world.get_mut::<Collider>(c) {
        col.density = 120.0;
    }
    y.name(c, "Catapulted crate");
    let parts = y.parts("steel_ball/0.6", [Mat::Steel], || [props::ball(0.6)]);
    let ball = y.steel_ball(base + DVec3::new(3.5, 9.0, 0.0), 0.6, parts);
    y.name(ball, "Seesaw ball");
}

/// A bin with a four-paddle wheel turned by a hinge motor, full of balls and blocks.
fn paddle_wheel(y: &mut Yard, center: DVec3) {
    for (dx, dz, hx, hz) in [
        (3.15, 0.0, 0.15, 2.8),
        (-3.15, 0.0, 0.15, 2.8),
        (0.0, 2.05, 3.0, 0.15),
        (0.0, -2.65, 3.0, 0.15),
    ] {
        y.block(center + DVec3::new(dx, 0.7, dz), Vec3::new(hx, 0.7, hz));
    }
    let axle = 2.0;
    let bin = y.parts("paddle_bin", [Mat::Concrete, Mat::Dark, Mat::Steel], || {
        let b = structures::paddle_bin(Vec2::new(3.0, 2.5), 0.3, 1.4, axle);
        [b.concrete, b.painted, b.steel]
    });
    y.scenery(center, Quat::IDENTITY, bin);
    let hub_at = center + DVec3::new(0.0, axle as f64, 0.0);
    let wheel = y.spawn((
        Transform::from_translation(y.at(hub_at)),
        RigidBody::dynamic(),
        Velocity::default(),
        Name::new("Paddle wheel"),
    ));
    let hub = y.parts("paddle_hub", [Mat::Yellow, Mat::Steel], || {
        let h = structures::paddle_hub(0.3, 2.7);
        [h.painted, h.steel]
    });
    y.dress(wheel, hub);
    let along_x = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
    y.spawn((
        Transform::default().with_rotation(along_x),
        Collider {
            shape: ColliderShape::Cylinder {
                radius: 0.3,
                half_height: 2.7,
            },
            surface: y.metal,
            density: 2000.0,
            ..Default::default()
        },
        ChildOf(wheel),
    ));
    let (half, reach) = (Vec3::new(2.6, 0.75, 0.05), 1.05);
    for k in 0..4 {
        let r = Quat::from_rotation_x(k as f32 * std::f32::consts::FRAC_PI_2);
        let paddle = y.spawn((
            Transform::from_translation((r * Vec3::new(0.0, reach, 0.0)).as_dvec3())
                .with_rotation(r),
            Collider::cuboid(half)
                .with_surface(y.wood)
                .with_density(600.0),
            ChildOf(wheel),
        ));
        let blade = y.parts(
            "paddle_blade",
            [Mat::Timber, Mat::Yellow, Mat::Steel],
            || {
                let b = structures::paddle_blade(half, reach);
                [b.timber, b.painted, b.steel]
            },
        );
        y.dress(paddle, blade);
    }
    y.joint(
        JointKind::Revolute {
            axis: Vec3::X,
            limits: None,
            motor: Some(JointMotor {
                target_velocity: 1.2,
                target_position: None,
                stiffness: 0.0,
                damping: 1000.0,
                max_force: 2.0e6,
            }),
        },
        wheel,
        None,
        hub_at,
    );
    // On the bin's floor, clear of the paddles: dropped onto the turning wheel from above,
    // they bounced out and rolled across the yard.
    let mut rng = Rng(77);
    let rows = [-2.15, -1.55, -0.95, 0.95, 1.55, 2.15];
    for i in 0..50 {
        let (col, row) = (i / rows.len(), i % rows.len());
        let at = center
            + DVec3::new(
                -2.45 + col as f64 * 0.6 + (rng.next() as f64 - 0.5) * 0.08,
                0.3,
                rows[row] + (rng.next() as f64 - 0.5) * 0.08,
            );
        if i % 3 == 0 {
            y.crate_box(
                at,
                Vec3::splat(0.22),
                Quat::from_rotation_y(rng.next() * TAU),
                i as u32 % CRATE_SEEDS,
            );
        } else {
            let color = [0xe0573a, 0x3a8fe0, 0xe0c03a][i % 3];
            y.ball(at, 0.25, 200.0, color);
        }
    }
}

/// Fifteen 0.82 m planks hinged together between two posts 12 m apart, so the deck sags about
/// 1.2 m; three crates drop onto it.
fn bridge(y: &mut Yard, center: DVec3) {
    let (span, planks, plank, height) = (12.0f64, 15usize, 0.82f64, 4.5f64);
    let tower_half = Vec3::new(1.3, height as f32 * 0.5, 0.3);
    for dz in [-1.0, 1.0] {
        let tower = y.block(
            center + DVec3::new(0.0, height * 0.5, dz * (span * 0.5 + 0.3)),
            tower_half,
        );
        // The deck is on the tower's inner side.
        let side = -dz as f32;
        let parts = y.parts(
            &format!("bridge_tower/{side}"),
            [Mat::Timber, Mat::Steel, Mat::Concrete],
            || {
                let t = structures::bridge_tower(tower_half, side);
                [t.timber, t.steel, t.concrete]
            },
        );
        y.dress(tower, parts);
    }
    // Hang the planks on a parabola whose arc length equals the deck length.
    let arc = |sag: f64| {
        let n = 2000;
        let p = |k: usize| {
            let z = -span * 0.5 + span * k as f64 / n as f64;
            DVec3::new(0.0, -sag * (1.0 - (2.0 * z / span).powi(2)), z)
        };
        let pts: Vec<DVec3> = (0..=n).map(p).collect();
        let mut lengths = vec![0.0];
        for w in pts.windows(2) {
            lengths.push(lengths.last().unwrap() + w[0].distance(w[1]));
        }
        (pts, lengths)
    };
    let deck = planks as f64 * plank;
    let (mut lo, mut hi) = (0.0, span);
    for _ in 0..60 {
        let mid = 0.5 * (lo + hi);
        if *arc(mid).1.last().unwrap() < deck {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let (pts, lengths) = arc(0.5 * (lo + hi));
    let point_at = |s: f64| {
        let i = lengths.partition_point(|l| *l < s).clamp(1, pts.len() - 1);
        let t = (s - lengths[i - 1]) / (lengths[i] - lengths[i - 1]).max(1e-12);
        pts[i - 1].lerp(pts[i], t)
    };
    let ends: Vec<DVec3> = (0..=planks)
        .map(|k| center + DVec3::Y * height + point_at(k as f64 * plank))
        .collect();
    let mut prev: Option<Entity> = None;
    for k in 0..planks {
        let (a, b) = (ends[k], ends[k + 1]);
        let dir = (b - a).normalize().as_vec3();
        let half = Vec3::new(1.0, 0.06, (a.distance(b) * 0.5) as f32);
        let c = Collider::cuboid(half)
            .with_surface(y.wood)
            .with_density(500.0);
        let parts = y.parts(
            &format!("bridge_plank/{k}"),
            [Mat::Timber, Mat::Steel],
            || {
                let p = structures::bridge_plank(half, k);
                [p.timber, p.steel]
            },
        );
        let plank = y.body(
            (a + b) * 0.5,
            Quat::from_rotation_arc(Vec3::Z, dir),
            c,
            parts,
        );
        let hinge = bridge_hinge();
        match prev {
            None => y.joint(hinge, plank, None, a),
            Some(p) => y.joint(hinge, p, Some(plank), a),
        };
        prev = Some(plank);
    }
    y.joint(bridge_hinge(), prev.unwrap(), None, ends[planks]);
    for (i, dz) in [-2.0, 0.5, 3.0].into_iter().enumerate() {
        y.crate_box(
            center + DVec3::new(0.0, 5.5 + i as f64, dz),
            Vec3::splat(0.35),
            Quat::from_rotation_y(0.4 * i as f32),
            i as u32 % CRATE_SEEDS,
        );
    }
    // Someone standing on the deck: the bridge sags under their weight too.
    y.mannequin(
        center + DVec3::new(0.0, height - 0.8, -0.8),
        0x2e86e8,
        "Bridge walker",
    );
}

/// A kinematic disc turning at 0.4 rad/s with crates riding on it.
/// A free hinge with a little friction, so the deck stops swinging like a real rope bridge.
fn bridge_hinge() -> JointKind {
    JointKind::Revolute {
        axis: Vec3::X,
        limits: None,
        motor: Some(JointMotor {
            target_velocity: 0.0,
            target_position: None,
            stiffness: 0.0,
            damping: 40.0,
            max_force: 400.0,
        }),
    }
}

fn turntable(y: &mut Yard, center: DVec3) {
    let table = y.spawn((
        Transform::from_translation(y.at(center + DVec3::Y * 0.15)),
        RigidBody::kinematic(),
        Collider {
            shape: ColliderShape::Cylinder {
                radius: 4.0,
                half_height: 0.15,
            },
            surface: y.concrete,
            ..Default::default()
        },
        Velocity {
            linear: Vec3::ZERO,
            angular: Vec3::new(0.0, 0.4, 0.0),
        },
        Name::new("Turntable"),
    ));
    let parts = y.parts("turntable", [Mat::Diamond, Mat::Yellow, Mat::Steel], || {
        let t = structures::turntable(4.0, 0.3);
        [t.deck, t.painted, t.steel]
    });
    y.dress(table, parts);
    y.mannequin(
        center + DVec3::new(1.2, 0.3, 0.0),
        0xe8772e,
        "Turntable rider",
    );
    for k in 0..10 {
        let a = k as f64 / 10.0 * std::f64::consts::TAU;
        let at = center + DVec3::new(a.cos() * 3.0, 0.3 + 0.35, a.sin() * 3.0);
        y.crate_box(
            at,
            Vec3::splat(0.35),
            Quat::from_rotation_y(-a as f32),
            k as u32 % CRATE_SEEDS,
        );
    }
}

/// Rubber balls dropped from different heights.
fn bouncy_balls(y: &mut Yard, center: DVec3) {
    let colors = [0xe0573a, 0x3a8fe0, 0xe0c03a, 0x6ac94a, 0xc04ad0, 0xf08a2a];
    for (i, color) in colors.into_iter().enumerate() {
        let at = center + DVec3::new(i as f64 * 1.6, 4.0 + i as f64 * 1.5, (i % 2) as f64 * 1.2);
        y.ball(at, 0.4, 300.0, color);
    }
}

/// The pendulum wave's cycle: every bob swings a whole number of times in it, so they all
/// come back into step, seconds.
pub const WAVE_CYCLE_SECS: f32 = 60.0;

/// The pendulum wave's bobs: bob `k` swings `WAVE_FIRST_SWINGS + k` times per cycle.
const WAVE_BOBS: usize = 15;
const WAVE_FIRST_SWINGS: usize = 20;
const WAVE_BOB_RADIUS: f32 = 0.1;
/// How far out the bobs start, radians.
const WAVE_AMPLITUDE: f32 = 0.2;

/// Pivot to center of a ball of `radius` that swings `swings` times per `cycle` seconds
/// when let go `amplitude` radians out.
pub fn wave_length(swings: usize, cycle: f32, radius: f32, amplitude: f32) -> f32 {
    let period = cycle / swings as f32;
    // A wider swing takes a little longer than a small one.
    let small = period / (1.0 + amplitude * amplitude / 16.0);
    let a = 9.81 * small * small / (4.0 * std::f32::consts::PI * std::f32::consts::PI);
    // A ball, not a point: L² - aL + 0.4r² = 0.
    (a + (a * a - 1.6 * radius * radius).sqrt()) / 2.0
}

/// An sRGB hex color of `hue` (0..1 around the wheel), as saturated as a lacquered toy.
fn hue_hex(hue: f32) -> u32 {
    let (s, v) = (0.78f32, 0.86f32);
    let h = (hue.fract() * 6.0).max(0.0);
    let i = h.floor() as u32 % 6;
    let f = h - h.floor();
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    let (r, g, b) = match i {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    let byte = |c: f32| (c.clamp(0.0, 1.0) * 255.0).round() as u32;
    (byte(r) << 16) | (byte(g) << 8) | byte(b)
}

/// Fifteen steel bobs under an A-frame gantry, swinging 20, 21 … 34 times a minute (lengths
/// from 2.2 m down to 0.77 m). Let go together, they fall out of step into traveling waves
/// and snakes, swing in two alternate rows at half a minute and are back in step at a
/// minute (`tests/pendulums.rs`).
fn pendulum_wave(y: &mut Yard, base: DVec3) {
    use art::pendulums::{self as pend, WAVE_EYE_DROP, WAVE_WIRE_SPREAD};
    let (height, pitch) = (3.25f32, 0.27f32);
    let first_x = -pitch * (WAVE_BOBS - 1) as f32 * 0.5;
    let (half_span, splay) = (2.3f32, 1.1f32);
    let frame = y.parts(
        "wave_frame",
        [Mat::Yellow, Mat::Steel, Mat::Concrete],
        || {
            let f = pend::wave_frame(height, half_span, splay, WAVE_BOBS, first_x, pitch);
            [f.painted, f.steel, f.concrete]
        },
    );
    y.scenery(base, Quat::IDENTITY, frame);
    for sx in [-1.0, 1.0] {
        for sz in [-1.0, 1.0] {
            y.block(
                base + DVec3::new(sx * half_span as f64, 0.1, sz * splay as f64),
                Vec3::new(0.24, 0.1, 0.24),
            );
        }
    }
    let pivot_y = (height - WAVE_EYE_DROP) as f64;
    let turn = Quat::from_rotation_x(WAVE_AMPLITUDE);
    for k in 0..WAVE_BOBS {
        let length = wave_length(
            WAVE_FIRST_SWINGS + k,
            WAVE_CYCLE_SECS,
            WAVE_BOB_RADIUS,
            WAVE_AMPLITUDE,
        );
        let pivot = base + DVec3::new((first_x + pitch * k as f32) as f64, pivot_y, 0.0);
        let center = pivot + (turn * Vec3::new(0.0, -length, 0.0)).as_dvec3();
        let color = hue_hex(k as f32 / WAVE_BOBS as f32 * 0.82);
        let parts = y.parts(
            &format!("wave_bob/{k}"),
            [Mat::Lacquer(color), Mat::Dark],
            || {
                let b = pend::bob(WAVE_BOB_RADIUS, length, Vec3::X, WAVE_WIRE_SPREAD);
                [b.ball, b.wire]
            },
        );
        let c = Collider::sphere(WAVE_BOB_RADIUS)
            .with_surface(y.metal)
            .with_density(7800.0);
        let bob = y.body(center, turn, c, parts);
        y.name(bob, &format!("Pendulum wave bob {}", k + 1));
        let hinge = JointKind::Revolute {
            axis: Vec3::X,
            limits: None,
            motor: None,
        };
        y.joint(hinge, bob, None, pivot);
    }
}

/// The Newton's cradle's balls: radius, pivot to center, and the gap between two at rest
/// (more than the solver's 2 mm contact prediction, so resting balls don't touch).
const CRADLE_BALL: f32 = 0.1;
const CRADLE_LENGTH: f32 = 0.8;
const CRADLE_GAP: f32 = 0.003;

/// A Newton's cradle on a concrete plinth: five 20 cm steel balls on 80 cm wires. The first
/// is let go 34° out and the blow passes down the row: the middle three stay put and the
/// last flies out, every swing. The balls step in fine substeps ([`crate::FineSteps`]),
/// without which the row swings off as one lump (`tests/pendulums.rs`).
fn newtons_cradle(y: &mut Yard, base: DVec3) {
    use art::pendulums as pend;
    let plinth = Vec3::new(1.0, 0.5, 0.6);
    let block = y.block(base + DVec3::Y * plinth.y as f64, plinth);
    let stone = y.parts("cradle_plinth", [Mat::Concrete], || {
        let mut b = MeshBuilder::new();
        b.rounded_box(plinth, 0.03, 2);
        [b.build()]
    });
    y.dress(block, stone);
    let top = base + DVec3::Y * (plinth.y * 2.0) as f64;
    // Board 8.2 cm, the balls' bottoms 8 cm above it.
    let pivot_y = 0.082 + 0.08 + CRADLE_BALL + CRADLE_LENGTH;
    let (half_x, half_z, bar_z) = (0.85f32, 0.42f32, 0.34f32);
    let frame = y.parts(
        "cradle_frame",
        [Mat::Lacquer(0x15161a), Mat::Chrome],
        || {
            let c = pend::cradle(half_x, half_z, bar_z, pivot_y);
            [c.base, c.chrome]
        },
    );
    y.scenery(top, Quat::IDENTITY, frame);
    let steel = {
        let mut table = y.world.get_resource_or_init::<SurfaceMaterials>();
        match table.id("hardened_steel") {
            Some(id) => id,
            None => table.add(SurfaceMaterial {
                name: "hardened_steel".into(),
                friction: 0.05,
                restitution: 0.97,
                density: 7800.0,
                footstep_sound: "footstep.metal".into(),
                impact_effect: "impact.metal".into(),
                ..Default::default()
            }),
        }
    };
    let pitch = 2.0 * CRADLE_BALL + CRADLE_GAP;
    for i in 0..5 {
        let pivot = top + DVec3::new(((i as f32 - 2.0) * pitch) as f64, pivot_y as f64, 0.0);
        // The first ball pulled out toward -x.
        let angle = if i == 0 { -0.6f32 } else { 0.0 };
        let turn = Quat::from_rotation_z(angle);
        let center = pivot + (turn * Vec3::new(0.0, -CRADLE_LENGTH, 0.0)).as_dvec3();
        let parts = y.parts("cradle_ball", [Mat::Chrome, Mat::Dark], || {
            let b = pend::bob(CRADLE_BALL, CRADLE_LENGTH, Vec3::Z, bar_z);
            [b.ball, b.wire]
        });
        let c = Collider::sphere(CRADLE_BALL)
            .with_surface(steel)
            .with_density(7800.0);
        let ball = y.body(center, turn, c, parts);
        y.world.entity_mut(ball).insert(crate::FineSteps::default());
        y.name(ball, &format!("Newton's cradle ball {}", i + 1));
        let hinge = JointKind::Revolute {
            axis: Vec3::Z,
            limits: None,
            motor: None,
        };
        y.joint(hinge, ball, None, pivot);
    }
}

/// Three oil drums roll down a ramp and along the runway into the sand pit. The ramp and the
/// runway are one triangle mesh, so the drums cross a triangle edge every 2 m at up to 5 m/s:
/// they stay flat and in their lanes (see `tests/rolling.rs`), and the sand's rolling
/// resistance stops them.
fn barrel_run(y: &mut Yard, base: DVec3) {
    use art::barrel_run::{self as run, *};
    let sand = y
        .world
        .get_resource_or_init::<SurfaceMaterials>()
        .id_or_default("sand");
    let cuboid = |x0: f32, x1: f32, y0: f32, y1: f32, half_z: f32| {
        (
            DVec3::new(((x0 + x1) * 0.5) as f64, ((y0 + y1) * 0.5) as f64, 0.0),
            Vec3::new((x1 - x0) * 0.5, (y1 - y0) * 0.5, half_z),
        )
    };
    let (vertices, indices) = run::deck_collider();
    let deck = y.fixed(
        base,
        Quat::IDENTITY,
        ColliderShape::TriMesh { vertices, indices },
        y.concrete,
    );
    y.name(deck, "Barrel run");
    // The sand pit (2 cm below the asphalt) and the tire wall.
    let (at, half) = cuboid(X_PIT, X_WALL, SAND_Y - 0.2, SAND_Y, HALF_WIDTH);
    y.fixed(
        base + at,
        Quat::IDENTITY,
        ColliderShape::Cuboid { half_extents: half },
        sand,
    );
    let (at, half) = cuboid(X_WALL, X_END, 0.0, WALL_HEIGHT, WALL_HALF);
    let rubber = y.rubber;
    y.fixed(
        base + at,
        Quat::IDENTITY,
        ColliderShape::Cuboid { half_extents: half },
        rubber,
    );
    // The ramp's body and its cheek walls, down to the yard, and guardrails a ball can't roll
    // under: balls roaming the yard stay out of the lanes.
    let profile = run::deck_profile(0.5);
    let hull = |outline: &[Vec2], z: f32, half_t: f32| {
        let points = outline
            .iter()
            .flat_map(|p| {
                [
                    Vec3::new(p.x, p.y, z - half_t),
                    Vec3::new(p.x, p.y, z + half_t),
                ]
            })
            .collect();
        ColliderShape::ConvexHull { points }
    };
    let concrete = y.concrete;
    let body = hull(&run::ramp_body_outline(), 0.0, HALF_WIDTH);
    y.fixed(base, Quat::IDENTITY, body, concrete);
    let cheek = run::cheek_outline(&profile);
    let metal = y.metal;
    let rail_top = RAIL_Y + 0.16;
    for s in [-1.0f32, 1.0] {
        let wall = hull(&cheek, s * (HALF_WIDTH + CHEEK_T * 0.5), CHEEK_T * 0.5);
        y.fixed(base, Quat::IDENTITY, wall, concrete);
        // The pit's curbs.
        let (at, half) = cuboid(X_PIT, X_WALL, 0.0, 0.12, 0.1);
        y.block(base + at + DVec3::Z * (s * (HALF_WIDTH + 0.1)) as f64, half);
        // The rails: straight along the deck, and the flare at their start.
        let (x0, x1) = run::rail_straight();
        let (at, half) = cuboid(x0, x1, 0.0, rail_top, 0.05);
        let shape = ColliderShape::Cuboid { half_extents: half };
        let z = (s * (RAIL_Z + 0.05)) as f64;
        y.fixed(base + at + DVec3::Z * z, Quat::IDENTITY, shape, metal);
        let (a, b) = run::rail_flare();
        let (a, b) = (Vec2::new(a.x, s * a.y), Vec2::new(b.x, s * b.y));
        let mid = (a + b) * 0.5;
        let half = Vec3::new(a.distance(b) * 0.5, rail_top * 0.5, 0.05);
        let turn = Quat::from_rotation_y(-(b.y - a.y).atan2(b.x - a.x));
        let at = DVec3::new(mid.x as f64, (rail_top * 0.5) as f64, mid.y as f64);
        let shape = ColliderShape::Cuboid { half_extents: half };
        y.fixed(base + at, turn, shape, metal);
    }
    let mats = [
        Mat::Asphalt,
        Mat::Concrete,
        Mat::Paint,
        Mat::Galvanized,
        Mat::Sand,
        Mat::Rubber(0xffffff),
        Mat::Yellow,
    ];
    let parts = y.parts("barrel_run", mats, || {
        let r = run::barrel_run();
        [
            r.asphalt,
            r.concrete,
            r.paint,
            r.galvanized,
            r.sand,
            r.tires,
            r.yellow,
        ]
    });
    y.scenery(base, Quat::IDENTITY, parts);
    // The drums, lying across their lanes on the slope just past the crest.
    let (radius, half_height) = (0.3, 0.42);
    for (lane, color) in (-1..=1).zip(DRUM_COLORS) {
        let c = Collider {
            shape: ColliderShape::Cylinder {
                radius,
                half_height,
            },
            surface: y.metal,
            density: 400.0,
            ..Default::default()
        };
        let parts = y.parts(&format!("drum/{color:06x}"), [Mat::Drum(color)], || {
            [props::steel_drum(radius, half_height)]
        });
        let at = base + run::barrel_start(lane, radius).as_dvec3();
        let lying = Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
        let drum = y.body(at, lying, c, parts);
        y.name(drum, &format!("Run drum {}", lane + 2));
    }
}

/// Rocks in the rockfall.
const ROCKFALL_ROCKS: usize = 300;

/// Rock shapes in the rockfall (each rock scales one of them).
const ROCK_SHAPES: u32 = 8;

/// Each rock shape's shade (sRGB hex, multiplying the granite).
const ROCK_TINTS: [u32; ROCK_SHAPES as usize] = [
    0xffffff, 0xe4e0da, 0xcfcbc5, 0xf2e4da, 0xc4c4c4, 0xe8dacf, 0xdadada, 0xbdb8b2,
];

/// The rockfall chute's slope, degrees: steeper than crushed rock's angle of repose on steel,
/// so it all slides off; the heap below takes the rock's own on concrete (about 35 degrees).
const CHUTE_DEG: f32 = 40.0;

/// A hopper on a steel chute, and [`ROCKFALL_ROCKS`] crushed granite rocks (30 to 55 cm, 2.6
/// t/m³) dropped into it: they pour out of the hopper's mouth, slide down the chute and drop
/// off its lip onto a heap. The chute runs down toward +X; `base` is under its lip.
fn rockfall(y: &mut Yard, base: DVec3) {
    // Crushed rock interlocks: its heaps stand at about 35 degrees on concrete.
    let rock = {
        let mut table = y.world.get_resource_or_init::<SurfaceMaterials>();
        let base = table.get(table.id_or_default("rock")).clone();
        table.add(SurfaceMaterial {
            name: "crushed_rock".into(),
            friction: 1.0,
            ..base
        })
    };
    // The chute's plates are worn smooth by the rock (friction 0.35; with the rock's 1.0, 0.68
    // between them: rock slides from 34 degrees). On plain steel (0.75 between them, 37
    // degrees) a few rocks wedged against a side plate and stopped on the 40-degree chute.
    let chute = {
        let mut table = y.world.get_resource_or_init::<SurfaceMaterials>();
        match table.id("worn_chute_steel") {
            Some(id) => id,
            None => {
                let base = table.get(y.metal).clone();
                table.add(SurfaceMaterial {
                    name: "worn_chute_steel".into(),
                    friction: 0.35,
                    ..base
                })
            }
        }
    };
    let (plates, floor_at, top) = chute_plates();
    for &(at, rotation, half, _) in &plates {
        y.fixed(
            base + at.as_dvec3(),
            rotation,
            ColliderShape::Cuboid { half_extents: half },
            chute,
        );
    }
    let art = y.parts(
        "rockfall_chute",
        [Mat::Steel, Mat::Yellow, Mat::Dark],
        || {
            let mut b: [MeshBuilder; 3] = Default::default();
            for (at, rotation, half, mat) in chute_plates().0 {
                let m = Mat4::from_rotation_translation(rotation, at);
                b[mat as usize].placed(m, |b| b.rounded_box(half, 0.012, 2));
            }
            b.map(MeshBuilder::build)
        },
    );
    y.scenery(base, Quat::IDENTITY, art);
    // The rocks, in layers over the hopper, 5 by 5 each 58 cm apart.
    let mut rng = Rng(0x0dd_5eed);
    for i in 0..ROCKFALL_ROCKS {
        let (gx, gz, layer) = ((i % 5) as f32, ((i / 5) % 5) as f32, (i / 25) as f32);
        let x = top.x + 0.35 + gx * 0.58;
        let at = Vec3::new(
            x + rng.next() * 0.06,
            floor_at(x) + 0.6 + layer * 0.58,
            (gz - 2.0) * 0.58 + rng.next() * 0.06,
        );
        // 30 to 55 cm across, never flatter than 2:1.
        let size = 0.3 + 0.25 * rng.next() * rng.next();
        let scale = Vec3::new(
            size * (0.8 + 0.4 * rng.next()),
            size * (0.6 + 0.3 * rng.next()),
            size * (0.8 + 0.4 * rng.next()),
        );
        let shape = (rng.next() * ROCK_SHAPES as f32) as u32 % ROCK_SHAPES;
        let rotation = Quat::from_euler(
            EulerRot::XYZ,
            rng.next() * TAU,
            rng.next() * TAU,
            rng.next() * TAU,
        );
        let c = Collider {
            shape: ColliderShape::ConvexHull {
                points: props::rock_points(shape),
            },
            surface: rock,
            density: 2600.0,
            ..Default::default()
        };
        let parts = y.parts(&format!("rock/{shape}"), [Mat::Granite], || {
            [props::rock(
                &props::rock_points(shape),
                ROCK_TINTS[shape as usize],
            )]
        });
        let e = y.body(base + at.as_dvec3(), rotation, c, parts);
        y.world.get_mut::<Transform>(e).unwrap().scale = scale;
    }
}

/// The rockfall's steel as boxes relative to the chute's lip: (center, rotation, half
/// extents, 0 steel / 1 yellow paint / 2 dark paint), each also a collider. Also the height of
/// the chute's floor at a given x, and its upper end.
#[allow(clippy::type_complexity)]
fn chute_plates() -> (Vec<(Vec3, Quat, Vec3, u8)>, impl Fn(f32) -> f32, Vec3) {
    let tilt = CHUTE_DEG.to_radians();
    let (len, half_w, t) = (5.0f32, 1.5f32, 0.08f32);
    // The floor runs from its lip, 1.6 m up, up and back along -X.
    let lip = Vec3::new(0.0, 1.6, 0.0);
    let down = Vec3::new(tilt.cos(), -tilt.sin(), 0.0);
    let top = lip - down * len;
    let along = Quat::from_rotation_z(-tilt);
    let normal = along * Vec3::Y;
    let floor_at = move |x: f32| top.y - (x - top.x) * tilt.tan();
    let mid = (lip + top) * 0.5;
    let mut out = vec![(
        mid - normal * t * 0.5,
        along,
        Vec3::new(len * 0.5, t * 0.5, half_w),
        0,
    )];
    // Side walls, 0.6 m tall, closing in from the hopper's width to 2.1 m at the lip, so the
    // rocks land in one heap rather than a long ridge.
    let narrow = 1.05f32;
    let skew = ((half_w - narrow) / len).atan();
    let wall_len = len / skew.cos();
    for s in [-1.0f32, 1.0] {
        let at = mid + normal * 0.3 + Vec3::Z * s * ((half_w + narrow) * 0.5 + t * 0.5);
        let rotation = along * Quat::from_rotation_y(s * skew);
        out.push((at, rotation, Vec3::new(wall_len * 0.5, 0.3, t * 0.5), 0));
    }
    // The hopper over the chute's upper end: 3 m square, its walls standing 2 m above the
    // chute, the downhill one stopping 1.1 m above it (the mouth).
    let (hop, hop_h, mouth) = (3.0f32, 2.0f32, 1.1f32);
    let front_x = top.x + hop;
    let rim = top.y + hop_h;
    let low = floor_at(front_x);
    out.push((
        Vec3::new(top.x - t * 0.5, (top.y - 0.3 + rim) * 0.5, 0.0),
        Quat::IDENTITY,
        Vec3::new(t * 0.5, (rim - top.y + 0.3) * 0.5, half_w + t),
        1,
    ));
    out.push((
        Vec3::new(front_x + t * 0.5, (low + mouth + rim) * 0.5, 0.0),
        Quat::IDENTITY,
        Vec3::new(t * 0.5, (rim - low - mouth) * 0.5, half_w + t),
        1,
    ));
    for s in [-1.0f32, 1.0] {
        out.push((
            Vec3::new(top.x + hop * 0.5, (low + rim) * 0.5, s * (half_w + t * 0.5)),
            Quat::IDENTITY,
            Vec3::new(hop * 0.5, (rim - low) * 0.5, t * 0.5),
            1,
        ));
    }
    // A dark steel angle around the hopper's rim, and a stiffening band around its walls.
    let (cx, hw) = (top.x + hop * 0.5, hop * 0.5 + t);
    for (y_at, size, mat) in [(rim, 0.07f32, 2u8), ((low + mouth + rim) * 0.5, 0.05, 1)] {
        for s in [-1.0f32, 1.0] {
            out.push((
                Vec3::new(cx + s * (hw + size * 0.5), y_at, 0.0),
                Quat::IDENTITY,
                Vec3::new(size * 0.5, size, half_w + t + size),
                mat,
            ));
            out.push((
                Vec3::new(cx, y_at, s * (half_w + t + size * 0.5)),
                Quat::IDENTITY,
                Vec3::new(hw, size, size * 0.5),
                mat,
            ));
        }
    }
    // Flat-bar stiffeners standing 10 cm proud of the hopper's walls every 75 cm, and dark
    // angle posts on its corners.
    let (rib, proud) = (0.03f32, 0.05f32);
    for k in 1..4 {
        let along = k as f32 * hop / 4.0 - hop * 0.5;
        for s in [-1.0f32, 1.0] {
            out.push((
                Vec3::new(cx + along, (low + rim) * 0.5, s * (half_w + t + proud)),
                Quat::IDENTITY,
                Vec3::new(rib, (rim - low) * 0.5 - 0.08, proud),
                1,
            ));
        }
        let back = top.y - 0.3;
        out.push((
            Vec3::new(top.x - t - proud, (back + rim) * 0.5, along),
            Quat::IDENTITY,
            Vec3::new(proud, (rim - back) * 0.5 - 0.08, rib),
            1,
        ));
        out.push((
            Vec3::new(front_x + t + proud, (low + mouth + rim) * 0.5, along),
            Quat::IDENTITY,
            Vec3::new(proud, (rim - low - mouth) * 0.5 - 0.08, rib),
            1,
        ));
    }
    for (x, bottom) in [(top.x - t, top.y - 0.3), (front_x + t, low)] {
        for s in [-1.0f32, 1.0] {
            out.push((
                Vec3::new(x, (bottom + rim) * 0.5, s * (half_w + t)),
                Quat::IDENTITY,
                Vec3::new(0.06, (rim - bottom) * 0.5, 0.06),
                2,
            ));
        }
    }
    // Legs (16 cm box section) under the floor's edges at the lip, the middle and under the
    // hopper, braced across at knee height.
    let leg = 0.08f32;
    for x in [lip.x - 0.35, mid.x, top.x + 0.25] {
        let h = floor_at(x) - t;
        for s in [-1.0f32, 1.0] {
            out.push((
                Vec3::new(x, h * 0.5, s * (half_w - leg)),
                Quat::IDENTITY,
                Vec3::new(leg, h * 0.5, leg),
                1,
            ));
        }
        out.push((
            Vec3::new(x, 0.7, 0.0),
            Quat::IDENTITY,
            Vec3::new(0.05, 0.06, half_w - 2.0 * leg),
            2,
        ));
    }
    (out, floor_at, top)
}
