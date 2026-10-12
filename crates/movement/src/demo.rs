//! The movement showcase: [`spawn_demo`] builds it around a point.
//!
//! * **Parkour course** (+Z side): a runner loops sprint → slide under a bar → wall-run →
//!   wall-jump → wall-run → mantle onto a tower → jump and double jump off it. Afterimages
//!   trail every fast move, so a still frame shows the route.
//! * **Duel ring** (−Z side), three rounds on a loop:
//!   1. *Blink*: an elite (tempo speed 2.5) dash-strikes an untrained player (level 1).
//!      The dash and three slashes land in well under a second; the player can't react.
//!   2. *Focus*: the same elite against the same player at level 10. When it attacks, the
//!      world slows to 1/5.5; the player sees it coming, dashes aside, and counters while the
//!      elite recovers.
//!   3. *Pack*: four ordinary grunts attack the level 10 player one after another. Each swing
//!      plays in slow motion and each one is cut down in a cinematic beat.
//!
//!   A column of ten cubes beside the ring lights up to the player's current level. The duel
//!   only runs while a camera is within [`DuelDirector::watch_radius`] of the ring, because
//!   its slow motion slows the whole world.
//!
//! The characters are capsule puppets (body, head, visor, sword) posed procedurally from the
//! movement state until the animation module provides skeletons. The bots that drive them
//! write `MoveIntent` exactly like player input or NPC AI would. Only one demo per world.

use std::f32::consts::FRAC_PI_2;

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_render::{Camera, MeshRenderer, Shape, StandardMaterial};
use engine_world::locomotion::{LocomotionMode, MoveIntent};
use engine_world::physics::{CharacterController, Collider, RigidBody};
use engine_world::{AttackEvent, AttackKind};

use crate::afterimage::Afterimages;
use crate::combat::{AttackPhase, CombatState, Fighter};
use crate::mover::{Mover, flat};
use crate::profile::MovementProfile;
use crate::tempo::{Perceiver, Tempo};

/// Where things are, relative to the demo origin (meters).
pub mod layout {
    /// Center of the duel ring.
    pub const RING: [f64; 3] = [0.0, 0.0, -14.0];
    pub const RING_RADIUS: f32 = 8.0;
    /// The runner's start.
    pub const RUN_START: [f64; 3] = [-23.0, 0.1, 12.0];
    /// Top of the tower the runner climbs (a mantle from the end of the second wall-run).
    pub const TOWER_TOP: f64 = 5.5;

    /// A static collider box of the demo, relative to the demo's origin: `center` and full
    /// `size` in meters. The set dressing ([`crate::look::course`]) is drawn around these.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct Block {
        pub center: [f64; 3],
        pub size: [f32; 3],
    }

    /// Runway pad along +X.
    pub const RUNWAY: Block = Block {
        center: [0.0, 0.05, 12.0],
        size: [56.0, 0.1, 10.0],
    };
    /// Slide bar (bottom at 1.15 m: too low to run under, high enough to slide under).
    pub const SLIDE_BAR: Block = Block {
        center: [-14.0, 1.35, 12.0],
        size: [0.3, 0.4, 7.0],
    };
    /// The slide bar's posts.
    pub const SLIDE_POSTS: [Block; 2] = [
        Block {
            center: [-14.0, 0.775, 8.5],
            size: [0.3, 1.55, 0.3],
        },
        Block {
            center: [-14.0, 0.775, 15.5],
            size: [0.3, 1.55, 0.3],
        },
    ];
    /// Wall A (right side, runner heading +X): run face at z = 14.5.
    pub const WALL_A: Block = Block {
        center: [-2.0, 2.6, 14.8],
        size: [12.0, 5.0, 0.6],
    };
    /// Wall B (left side): run face at z = 9.5, runs into the tower.
    pub const WALL_B: Block = Block {
        center: [12.5, 2.6, 9.2],
        size: [13.0, 5.0, 0.6],
    };
    /// The tower: climbed by a mantle off wall B.
    pub const TOWER: Block = Block {
        center: [22.0, TOWER_TOP * 0.5, 11.0],
        size: [6.0, TOWER_TOP as f32, 7.0],
    };
    /// The ring platform (a 16 m square, 0.1 m high).
    pub const RING_PLATFORM: Block = Block {
        center: [RING[0], RING[1] + 0.05, RING[2]],
        size: [2.0 * RING_RADIUS, 0.1, 2.0 * RING_RADIUS],
    };
    /// Stands behind the ring for fighters sitting a round out.
    pub const STANDS: Block = Block {
        center: [RING[0], RING[1] + 0.15, RING[2] - 11.5],
        size: [16.0, 0.3, 3.0],
    };
    /// Center of the lowest level cell; the other nine stack [`LEVEL_STEP`] apart above it.
    pub const LEVEL_METER: [f64; 3] = [RING[0] + 9.5, RING[1] + 0.35, RING[2]];
    /// Spacing of the level cells, meters.
    pub const LEVEL_STEP: f64 = 0.5;
}

/// Level the duel player has in the blink round and in the focus/pack rounds.
pub const LEVEL_UNTRAINED: u32 = 1;
pub const LEVEL_TRAINED: u32 = 10;
/// Human reaction time the duel player bot waits before responding, real seconds.
pub const REACTION_SECS: f32 = 0.25;

/// Rounds of the duel, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Round {
    /// Level 1 player vs the elite.
    Blink,
    /// Level 10 player vs the elite.
    Focus,
    /// Level 10 player vs four grunts.
    Pack,
}

impl Round {
    fn next(self) -> Self {
        match self {
            Round::Blink => Round::Focus,
            Round::Focus => Round::Pack,
            Round::Pack => Round::Blink,
        }
    }
}

/// Runs the duel ring: sets up each round and cues the attackers.
#[derive(Component, Debug, Clone, Serialize, Deserialize)]
pub struct DuelDirector {
    pub round: Round,
    /// Real seconds since the round started.
    pub secs: f32,
    /// Rounds played so far.
    pub rounds_played: u32,
    /// Real seconds after the round's fight is decided before the next round.
    pub outro_secs: f32,
    /// Only play this round (tests and screenshots); `None` = cycle.
    pub only: Option<Round>,
    /// The duel waits (and its slow motion stays off) while every active camera is farther
    /// than this from the ring, meters. Slow motion is world-wide, so a duel nobody watches
    /// shouldn't slow the rest of the world.
    pub watch_radius: f32,
    #[serde(skip)]
    pub(crate) origin: DVec3,
    #[serde(skip)]
    pub player: Option<Entity>,
    #[serde(skip)]
    pub elite: Option<Entity>,
    #[serde(skip)]
    pub grunts: Vec<Entity>,
    #[serde(skip)]
    cubes: Vec<Entity>,
    decided_at: Option<f32>,
    set_up: bool,
}

impl DuelDirector {
    /// Start a round over from its setup: `Some(round)` plays only that round on a loop,
    /// `None` cycles through all three again (from the current round).
    pub fn restart(&mut self, round: Option<Round>) {
        if let Some(r) = round {
            self.round = r;
        }
        self.only = round;
        self.secs = 0.0;
        self.decided_at = None;
        self.set_up = false;
    }
}

/// What a demo bot does.
#[derive(Component, Debug, Clone, Serialize, Deserialize)]
pub enum Brain {
    /// Loops the parkour course.
    Runner {
        step: usize,
        secs: f32,
        #[serde(skip)]
        origin: DVec3,
    },
    /// Reacts to attacks after a human reaction time: dodge dashes, then counter.
    Duelist(DuelistBrain),
    /// Attacks the duel player when cued: a dash-strike, then a short combo.
    Attacker(AttackerBrain),
    /// Stands still.
    Idle,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct DuelistBrain {
    #[serde(skip)]
    threat: Option<Entity>,
    /// Real seconds when the current threat was seen.
    seen_at: f32,
    hits_at_threat: u32,
    dodged: bool,
    countered: bool,
    /// Real seconds since the brain started (its own clock).
    clock: f32,
    /// Real seconds after the threat when the player first acted (for logs and tests).
    pub reacted_after: Option<f32>,
    /// Real seconds from the attack telegraph to the player being hit, if it was.
    pub hit_after: Option<f32>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct AttackerBrain {
    /// Start attacking at this director time (real seconds); `None` = wait.
    pub engage_at: Option<f32>,
    pub engaged: bool,
    /// Follow-up swings after the opener.
    pub combo: u8,
    /// Open with a dash-strike (elite) or by running in (grunt).
    pub dash_opener: bool,
    #[serde(skip)]
    pulse: bool,
}

/// A procedurally posed stand-in body. Children: the rig (tilts), and the sword.
#[derive(Component, Debug, Clone, Copy)]
pub struct Puppet {
    rig: Entity,
    sword: Entity,
    tilt: Quat,
    crouch: f32,
    fallen: f32,
    swing: f32,
}

#[derive(Component)]
struct PuppetPart;

#[derive(Component, Debug, Clone, Copy)]
struct LevelCube(u32);

fn mat(hex: u32, rough: f32) -> StandardMaterial {
    StandardMaterial::hex(hex).with_roughness(rough)
}

fn glow(hex: u32, strength: f32) -> StandardMaterial {
    let c = engine_render::material::srgb_hex_to_linear(hex);
    StandardMaterial::hex(hex)
        .with_roughness(0.4)
        .with_emissive(c, strength)
}

/// A static box collider (no mesh: [`crate::look::course`] draws the set around it).
fn block(commands: &mut Commands, origin: DVec3, name: &'static str, b: layout::Block) {
    commands.spawn((
        Name::new(name),
        Transform::from_translation(origin + DVec3::from_array(b.center)),
        RigidBody::fixed(),
        Collider::cuboid(Vec3::from(b.size) * 0.5),
    ));
}

fn at(origin: DVec3, x: f64, y: f64, z: f64) -> DVec3 {
    origin + DVec3::new(x, y, z)
}

/// Colors of a character.
struct Look {
    body: StandardMaterial,
    head: StandardMaterial,
    visor: StandardMaterial,
    blade: StandardMaterial,
    trail: [f32; 3],
}

fn spawn_character(
    commands: &mut Commands,
    name: &str,
    pos: DVec3,
    facing: Vec3,
    look: &Look,
    profile: MovementProfile,
    extra: impl Bundle,
) -> Entity {
    let rotation = engine_core::transform::look_rotation(facing, Vec3::Y).unwrap_or_default();
    let id = commands
        .spawn((
            Name::new(name.to_string()),
            Transform::from_translation(pos).with_rotation(rotation),
            profile,
            CharacterController::default(),
            Afterimages::colored(look.trail).lasting(0.8),
            extra,
        ))
        .id();
    let rig = commands
        .spawn((Transform::default(), ChildOf(id), PuppetPart))
        .id();
    let part = |commands: &mut Commands, t: Transform, shape: Shape, m: StandardMaterial| {
        commands.spawn((t, MeshRenderer::new(shape, m), ChildOf(rig)));
    };
    part(
        commands,
        Transform::from_xyz(0.0, -0.22, 0.0),
        Shape::Capsule {
            radius: 0.3,
            height: 1.36,
            segments: 20,
        },
        look.body,
    );
    part(
        commands,
        Transform::from_xyz(0.0, 0.68, 0.0),
        Shape::Sphere {
            radius: 0.17,
            segments: 20,
        },
        look.head,
    );
    part(
        commands,
        Transform::from_xyz(0.0, 0.7, -0.14),
        Shape::Cuboid {
            size: [0.24, 0.06, 0.06],
        },
        look.visor,
    );
    let sword = commands
        .spawn((
            Transform::from_xyz(0.36, 0.05, -0.1),
            ChildOf(rig),
            PuppetPart,
        ))
        .id();
    commands.spawn((
        Transform::from_xyz(0.0, 0.0, -0.55),
        MeshRenderer::new(
            Shape::Cuboid {
                size: [0.04, 0.09, 1.1],
            },
            look.blade,
        )
        .without_shadows(),
        ChildOf(sword),
    ));
    commands.entity(id).insert(Puppet {
        rig,
        sword,
        tilt: Quat::IDENTITY,
        crouch: 0.0,
        fallen: 0.0,
        swing: 0.0,
    });
    id
}

/// Build the movement showcase around `origin` (ground level). See the module docs.
pub fn spawn_demo(commands: &mut Commands, origin: DVec3) {
    // Ground under the whole demo (physics only: the game draws its own ground).
    commands.spawn((
        Transform::from_translation(at(origin, 0.0, -0.5, 0.0)),
        RigidBody::fixed(),
        Collider::cuboid(Vec3::new(40.0, 0.5, 40.0)),
    ));

    // ---------------------------------------------------------------- parkour course
    block(commands, origin, "Runway", layout::RUNWAY);
    block(commands, origin, "Slide bar", layout::SLIDE_BAR);
    for post in layout::SLIDE_POSTS {
        block(commands, origin, "Slide bar post", post);
    }
    block(commands, origin, "Wall A", layout::WALL_A);
    block(commands, origin, "Wall B", layout::WALL_B);
    block(commands, origin, "Tower", layout::TOWER);
    // The pilot training facility drawn around those boxes.
    crate::look::course::dress_course(commands, origin);

    let runner_look = Look {
        body: mat(0x14a38b, 0.5),
        head: mat(0xe8d5c0, 0.6),
        visor: glow(0x7dffe0, 300.0),
        blade: glow(0x7dffe0, 300.0),
        trail: [0.2, 1.0, 0.75],
    };
    spawn_character(
        commands,
        "Runner",
        origin + DVec3::from_array(layout::RUN_START) + DVec3::Y * 0.9,
        Vec3::X,
        &runner_look,
        MovementProfile::pilot(),
        Brain::Runner {
            step: 0,
            secs: 0.0,
            origin,
        },
    );

    // ---------------------------------------------------------------- duel ring
    let ring = origin + DVec3::from_array(layout::RING);
    block(commands, origin, "Ring", layout::RING_PLATFORM);
    // Stands behind the ring for fighters sitting a round out.
    block(commands, origin, "Stands", layout::STANDS);
    // The samurai duel arena drawn around them.
    crate::look::course::dress_ring(commands, ring);
    // Level meter: ten cells in a shrine column, lit to the player's level.
    let meter = origin + DVec3::from_array(layout::LEVEL_METER);
    crate::look::course::dress_level_column(commands, DVec3::new(meter.x, ring.y, meter.z));
    let mut cubes = Vec::new();
    for i in 0..10 {
        let id = commands
            .spawn((
                Transform::from_translation(meter + DVec3::Y * (i as f64 * layout::LEVEL_STEP)),
                MeshRenderer::new(crate::look::course::level_cell(), mat(0x34363b, 0.8)),
                LevelCube(i),
            ))
            .id();
        cubes.push(id);
    }

    let player = spawn_character(
        commands,
        "Duelist",
        ring + DVec3::new(0.0, 1.0, 4.5),
        Vec3::NEG_Z,
        &Look {
            body: mat(0x2a6fdb, 0.45),
            head: mat(0xe8d5c0, 0.6),
            visor: glow(0x9fe8ff, 300.0),
            blade: glow(0x9fe8ff, 600.0),
            trail: [0.3, 0.75, 1.0],
        },
        MovementProfile::pilot(),
        (
            Fighter {
                damage: 30.0,
                ..Fighter::new(1, 100.0)
            },
            Tempo::level(LEVEL_UNTRAINED),
            Perceiver,
            Brain::Duelist(DuelistBrain::default()),
        ),
    );
    let elite = spawn_character(
        commands,
        "Elite",
        ring + DVec3::new(0.0, 1.0, -4.5),
        Vec3::Z,
        &Look {
            body: mat(0x2a0a10, 0.35),
            head: mat(0x1a1a1e, 0.4),
            visor: glow(0xff2a3a, 600.0),
            blade: glow(0xff2a3a, 900.0),
            trail: [1.0, 0.12, 0.18],
        },
        MovementProfile::elite(),
        (
            Fighter {
                damage: 35.0,
                reach: 1.7,
                dash_strike_range: 12.0,
                ..Fighter::new(2, 50.0)
            },
            Tempo {
                speed: 2.5,
                reflexes: 1.0,
            },
            Brain::Attacker(AttackerBrain {
                combo: 2,
                dash_opener: true,
                ..Default::default()
            }),
        ),
    );
    let mut grunts = Vec::new();
    for i in 0..4 {
        let g = spawn_character(
            commands,
            &format!("Grunt {}", i + 1),
            stand_spot(ring, i),
            Vec3::Z,
            &Look {
                body: mat(0xa0522d, 0.7),
                head: mat(0xd9b99b, 0.6),
                visor: glow(0xffa040, 200.0),
                blade: mat(0xb8bcc4, 0.3).with_metallic(1.0),
                trail: [1.0, 0.5, 0.2],
            },
            MovementProfile::human(),
            (
                Fighter {
                    auto_dash_strike: false,
                    ..Fighter::new(2, 25.0)
                },
                Tempo::default(),
                Brain::Attacker(AttackerBrain::default()),
            ),
        );
        grunts.push(g);
    }
    commands.spawn((
        Name::new("Duel director"),
        Transform::from_translation(ring),
        DuelDirector {
            round: Round::Blink,
            secs: 0.0,
            rounds_played: 0,
            outro_secs: 2.0,
            only: None,
            watch_radius: 70.0,
            origin: ring,
            player: Some(player),
            elite: Some(elite),
            grunts,
            cubes,
            decided_at: None,
            set_up: false,
        },
    ));
}

/// Where grunt `i` waits in the stands.
fn stand_spot(ring: DVec3, i: usize) -> DVec3 {
    ring + DVec3::new(-6.0 + i as f64 * 1.6, 1.2, -11.5)
}

pub(crate) fn register(app: &mut App) {
    app.register_component::<DuelDirector>(
        "Movement demo duel: round (Blink, Focus, Pack), seconds into it, rounds played; set \
         `only` to repeat one round.",
    )
    .add_systems(
        FixedUpdate,
        (direct_duel, run_runner, think_duelists, think_attackers)
            .chain()
            .in_set(SimSet::Input)
            .before(crate::MovementSet::Intent),
    )
    .add_systems(Update, animate_puppets);
}

// ------------------------------------------------------------------------ the runner

#[derive(Debug, Clone, Copy, PartialEq)]
enum Act {
    /// Stand at the start (teleported there) for a moment.
    Start(f32),
    Sprint,
    /// Hold crouch: slide.
    Slide,
    /// Press jump once, then steer.
    Jump,
    /// Steer along a wall.
    WallRun,
    /// Press jump on the wall.
    WallJump,
    /// Keep going; press jump again (double jump) once falling.
    DoubleJump,
    /// Run out and wait before looping.
    Finish(f32),
}

/// The route: (target x, target z, what to do on the way there), relative to the origin.
const ROUTE: &[(f64, f64, Act)] = &[
    (-23.0, 12.0, Act::Start(0.6)),
    (-17.5, 12.0, Act::Sprint),
    (-11.0, 12.0, Act::Slide),
    (-8.2, 13.0, Act::Sprint),
    (-5.0, 14.2, Act::Jump),
    (1.5, 14.2, Act::WallRun),
    (8.5, 10.2, Act::WallJump),
    (19.5, 9.9, Act::WallRun),
    (23.0, 11.0, Act::Sprint),
    (31.0, 11.0, Act::DoubleJump),
    (33.0, 11.0, Act::Finish(0.8)),
];

#[allow(clippy::type_complexity)]
fn run_runner(
    time: Res<Time>,
    mut q: Query<(
        &mut Brain,
        &mut Transform,
        &mut Mover,
        &mut MoveIntent,
        Option<&mut TransformInterpolation>,
    )>,
) {
    let real = time.real_delta_secs();
    for (mut brain, mut t, mut m, mut intent, interp) in &mut q {
        let Brain::Runner { step, secs, origin } = &mut *brain else {
            continue;
        };
        *secs += real;
        let (tx, tz, act) = ROUTE[*step % ROUTE.len()];
        let target = *origin + DVec3::new(tx, 0.0, tz);
        let pos = t.translation;
        let to = flat((target - pos).as_vec3(), Vec3::Y);
        let dir = to.normalize_or_zero();
        *intent = MoveIntent {
            direction: dir,
            sprint: true,
            ..Default::default()
        };
        // Passed the target (or close to it)?
        let prev = ROUTE[(*step + ROUTE.len() - 1) % ROUTE.len()];
        let seg = flat(
            (target - (*origin + DVec3::new(prev.0, 0.0, prev.1))).as_vec3(),
            Vec3::Y,
        )
        .normalize_or_zero();
        let reached = to.length() < 0.8 || (seg != Vec3::ZERO && to.dot(seg) < 0.0);
        let advance = match act {
            Act::Start(wait) => {
                if *secs < 0.02 {
                    t.translation = *origin + DVec3::from_array(layout::RUN_START) + DVec3::Y * 0.9;
                    t.rotation = Quat::from_rotation_y(-FRAC_PI_2);
                    m.reset_motion();
                    m.facing = Vec3::X;
                    if let Some(mut i) = interp {
                        i.reset();
                    }
                }
                intent.direction = Vec3::ZERO;
                intent.look = Some(Vec3::X);
                *secs >= wait
            }
            Act::Sprint => reached,
            Act::Slide => {
                intent.crouch = true;
                reached
            }
            Act::Jump => {
                intent.jump = *secs < 0.1;
                reached || m.mode == LocomotionMode::WallRun
            }
            Act::WallRun => {
                // Lean into the wall a little so the run sticks.
                if let Some(w) = m.wall {
                    intent.direction = (dir - w.normal * 0.3).normalize_or_zero();
                }
                reached || (*secs > 0.3 && m.mode == LocomotionMode::Mantle)
            }
            Act::WallJump => {
                intent.jump = *secs < 0.1;
                reached || (*secs > 0.3 && m.mode == LocomotionMode::WallRun)
            }
            Act::DoubleJump => {
                // Jump off the tower's edge, double jump at the top of the arc.
                let edge = m.grounded && (pos.x - origin.x) > 24.2;
                let apex = !m.grounded && m.velocity.y < 0.5 && m.air_jumps_left > 0;
                intent.jump = edge || apex;
                reached || (*secs > 0.5 && m.grounded && (pos.x - origin.x) > 26.0)
            }
            Act::Finish(wait) => {
                intent.direction = if reached { Vec3::ZERO } else { dir };
                intent.sprint = false;
                *secs >= wait + 1.5 || (reached && *secs >= wait)
            }
        };
        // Give up on a step that takes far too long (stuck): restart the loop.
        if *secs > 6.0 {
            *step = 0;
            *secs = 0.0;
            continue;
        }
        if advance {
            *step = (*step + 1) % ROUTE.len();
            *secs = 0.0;
        }
    }
}

// ------------------------------------------------------------------------ the duel

type Fighters<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut Transform,
        &'static mut Mover,
        &'static mut Fighter,
        &'static mut CombatState,
        &'static mut Tempo,
        &'static mut Brain,
        Option<&'static mut TransformInterpolation>,
        Option<&'static mut engine_world::animation::Ragdoll>,
    ),
>;

/// Revive a fighter and stand it at `pos`, facing `facing`.
fn place(fighters: &mut Fighters, e: Entity, pos: DVec3, facing: Vec3) {
    let Ok((mut t, mut m, mut fighter, mut cs, _, _, interp, ragdoll)) = fighters.get_mut(e) else {
        return;
    };
    cs.revive(&mut fighter, &mut m);
    cs.attack = None;
    cs.hits_taken = 0;
    cs.hits_landed = 0;
    m.reset_motion();
    m.facing = facing;
    t.translation = pos;
    if let Some(r) = engine_core::transform::look_rotation(facing, Vec3::Y) {
        t.rotation = r;
    }
    if let Some(mut i) = interp {
        i.reset();
    }
    if let Some(mut r) = ragdoll {
        r.weight = 0.0;
    }
}

#[allow(clippy::type_complexity)]
fn direct_duel(
    time: Res<Time>,
    cameras: Query<(&Camera, &GlobalTransform)>,
    mut directors: Query<&mut DuelDirector>,
    mut fighters: Fighters,
    mut cubes: Query<(&LevelCube, &mut MeshRenderer)>,
) {
    let real = time.real_delta_secs();
    for mut d in &mut directors {
        let (Some(player), Some(elite)) = (d.player, d.elite) else {
            continue;
        };
        let ring = d.origin;
        let nearest = cameras
            .iter()
            .filter(|(c, _)| c.active)
            .map(|(_, t)| t.translation().distance(ring))
            .reduce(f64::min);
        if nearest.is_some_and(|n| n > d.watch_radius as f64) {
            // Nobody is watching: reset once and wait.
            if d.set_up {
                set_up_round(&mut d, &mut fighters, &mut cubes);
                d.set_up = false;
            }
            d.secs = 0.0;
            d.decided_at = None;
            continue;
        }
        d.secs += real;
        if !d.set_up {
            set_up_round(&mut d, &mut fighters, &mut cubes);
            d.set_up = true;
        }
        // Decided? Player down, or every attacker in this round down.
        let player_down = fighters.get(player).is_ok_and(|f| f.3.defeated);
        let attackers: Vec<Entity> = match d.round {
            Round::Blink | Round::Focus => vec![elite],
            Round::Pack => d.grunts.clone(),
        };
        let attackers_down = attackers
            .iter()
            .all(|e| fighters.get(*e).is_ok_and(|f| f.3.defeated));
        let finished_combo = d.round == Round::Blink
            && d.secs > 1.0
            && fighters.get(elite).is_ok_and(|f| {
                matches!(f.5, Brain::Attacker(a) if a.engaged && a.combo == 0)
                    && f.3.attack.is_none()
            });
        if d.decided_at.is_none() && (player_down || attackers_down || finished_combo) {
            d.decided_at = Some(d.secs);
        }
        let limit = match d.round {
            Round::Blink => 6.0,
            Round::Focus => 10.0,
            Round::Pack => 18.0,
        };
        if d.decided_at.is_some_and(|t| d.secs - t >= d.outro_secs) || d.secs > limit {
            d.round = d.only.unwrap_or(d.round.next());
            d.secs = 0.0;
            d.decided_at = None;
            d.rounds_played += 1;
            set_up_round(&mut d, &mut fighters, &mut cubes);
        }
    }
}

#[allow(clippy::type_complexity)]
fn set_up_round(
    d: &mut DuelDirector,
    fighters: &mut Fighters,
    cubes: &mut Query<(&LevelCube, &mut MeshRenderer)>,
) {
    let ring = d.origin;
    let level = if d.round == Round::Blink {
        LEVEL_UNTRAINED
    } else {
        LEVEL_TRAINED
    };
    let player = d.player.expect("player");
    let elite = d.elite.expect("elite");
    place(
        fighters,
        player,
        ring + DVec3::new(0.0, 1.0, 4.5),
        Vec3::NEG_Z,
    );
    if let Ok((_, _, _, _, mut tempo, mut brain, _, _)) = fighters.get_mut(player) {
        *tempo = Tempo::level(level);
        *brain = Brain::Duelist(DuelistBrain::default());
    }
    let elite_spot = if d.round == Round::Pack {
        ring + DVec3::new(4.0, 1.2, -11.5)
    } else {
        ring + DVec3::new(0.0, 1.0, -4.5)
    };
    place(fighters, elite, elite_spot, Vec3::Z);
    if let Ok((_, _, _, _, _, mut brain, _, _)) = fighters.get_mut(elite) {
        *brain = Brain::Attacker(AttackerBrain {
            engage_at: (d.round != Round::Pack).then_some(1.0),
            combo: if d.round == Round::Blink { 2 } else { 0 },
            dash_opener: true,
            ..Default::default()
        });
    }
    let grunts = d.grunts.clone();
    for (i, g) in grunts.iter().enumerate() {
        let (pos, facing) = if d.round == Round::Pack {
            let a = i as f64 / grunts.len() as f64 * std::f64::consts::TAU + 0.6;
            let p = ring + DVec3::new(a.cos() * 6.5, 1.0, 4.5 + a.sin() * 6.5 - 4.5);
            (p, flat((ring - p).as_vec3(), Vec3::Y).normalize_or_zero())
        } else {
            (stand_spot(ring, i), Vec3::Z)
        };
        place(fighters, *g, pos, facing);
        if let Ok((_, _, _, _, _, mut brain, _, _)) = fighters.get_mut(*g) {
            *brain = Brain::Attacker(AttackerBrain {
                engage_at: (d.round == Round::Pack).then_some(1.0 + i as f32 * 1.8),
                ..Default::default()
            });
        }
    }
    for (cube, mut r) in cubes.iter_mut() {
        if !d.cubes.is_empty() {
            r.material = if cube.0 < level {
                glow(0x3fd0ff, 500.0)
            } else {
                mat(0x34363b, 0.8)
            }
            .into();
        }
    }
}

/// The duel player: waits a human reaction time after an attack starts, then sidesteps a
/// dash-strike (or cuts first at a plain swing in reach), then counters.
#[allow(clippy::type_complexity)]
fn think_duelists(
    time: Res<Time>,
    mut attacks: MessageReader<AttackEvent>,
    others: Query<(&Transform, &CombatState, &Mover), Without<Perceiver>>,
    mut q: Query<
        (
            Entity,
            &Transform,
            &Mover,
            &CombatState,
            &mut Brain,
            &mut MoveIntent,
        ),
        With<Perceiver>,
    >,
) {
    let events: Vec<AttackEvent> = attacks.read().copied().collect();
    let real = time.real_delta_secs();
    for (me, t, m, cs, mut brain, mut intent) in &mut q {
        let Brain::Duelist(b) = &mut *brain else {
            continue;
        };
        b.clock += real;
        for ev in events.iter().filter(|e| e.target == Some(me)) {
            if b.threat != Some(ev.attacker) || b.countered {
                b.threat = Some(ev.attacker);
                b.seen_at = b.clock;
                b.hits_at_threat = cs.hits_taken;
                b.dodged = false;
                b.countered = false;
            }
        }
        *intent = MoveIntent::default();
        // Face the nearest standing enemy.
        let nearest = others
            .iter()
            .filter(|(_, c, _)| !c.defeated)
            .map(|(ot, _, _)| ot.translation)
            .filter(|p| p.distance(t.translation) < 12.0)
            .min_by(|a, b| {
                a.distance(t.translation)
                    .total_cmp(&b.distance(t.translation))
            });
        if let Some(p) = nearest {
            intent.look = flat((p - t.translation).as_vec3(), Vec3::Y).try_normalize();
        }
        if cs.defeated {
            continue;
        }
        let Some(threat) = b.threat else {
            continue;
        };
        let Ok((tt, tcs, tm)) = others.get(threat) else {
            b.threat = None;
            continue;
        };
        if cs.hits_taken > b.hits_at_threat && b.hit_after.is_none() && !b.dodged {
            b.hit_after = Some(b.clock - b.seen_at);
        }
        if tcs.defeated {
            b.threat = None;
            continue;
        }
        let since = b.clock - b.seen_at;
        if since < REACTION_SECS || cs.hits_taken > b.hits_at_threat || cs.stagger_secs > 0.0 {
            continue;
        }
        let to = flat((tt.translation - t.translation).as_vec3(), Vec3::Y);
        let dist = to.length();
        let incoming = tcs.attacking();
        intent.look = to.try_normalize();
        if incoming && !b.dodged {
            b.reacted_after.get_or_insert(since);
            let dash_strike = tcs.attack.is_some_and(|a| a.kind == AttackKind::Dash);
            if !dash_strike && dist < 2.6 && tm.mode != LocomotionMode::Dash {
                // A plain swing already in reach: cut first.
                intent.attack = Some(AttackKind::Melee);
                b.dodged = true;
            } else {
                // Sidestep the dash-strike: 6 m to the right of the attacker's approach.
                let side = to.normalize_or_zero().cross(Vec3::Y);
                intent.dash_to = Some(t.translation + (side * 6.0).as_dvec3());
                b.dodged = true;
            }
        } else if b.dodged
            && !b.countered
            && m.dash.is_none()
            && cs.attack.is_none()
            && (tcs.vulnerable() || !incoming)
        {
            intent.attack = Some(AttackKind::Melee);
            b.countered = true;
        }
    }
}

/// Attackers: engage when cued (dash-strike opener or run in), then follow up with a combo.
#[allow(clippy::type_complexity)]
fn think_attackers(
    directors: Query<&DuelDirector>,
    targets: Query<(&Transform, &CombatState), With<Perceiver>>,
    mut q: Query<(
        &Transform,
        &Mover,
        &Fighter,
        &CombatState,
        &mut Brain,
        &mut MoveIntent,
    )>,
) {
    let Some(d) = directors.iter().next() else {
        return;
    };
    let Some((pt, pcs)) = d.player.and_then(|p| targets.get(p).ok()) else {
        return;
    };
    for (t, m, f, cs, mut brain, mut intent) in &mut q {
        let Brain::Attacker(b) = &mut *brain else {
            continue;
        };
        let to = flat((pt.translation - t.translation).as_vec3(), Vec3::Y);
        let dist = to.length();
        // Clear last step's one-shot presses so the next press is a fresh edge.
        let pulsed = std::mem::take(&mut b.pulse);
        *intent = MoveIntent {
            look: to.try_normalize(),
            ..Default::default()
        };
        if cs.defeated || pcs.defeated || pulsed {
            continue;
        }
        let Some(at) = b.engage_at else {
            continue;
        };
        if d.secs < at {
            continue;
        }
        if !b.engaged {
            if b.dash_opener {
                intent.attack = Some(AttackKind::Dash);
                intent.dash_to = Some(pt.translation);
                b.engaged = true;
                b.pulse = true;
            } else if dist > f.reach + 0.5 {
                intent.direction = to.normalize_or_zero();
                intent.sprint = dist > 4.0;
            } else {
                intent.attack = Some(AttackKind::Melee);
                b.engaged = true;
                b.pulse = true;
            }
            continue;
        }
        // Follow-up swings while the target is in reach.
        if cs.attack.is_none() && cs.stagger_secs <= 0.0 && m.dash.is_none() && b.combo > 0 {
            if dist <= f.reach + 0.8 {
                intent.attack = Some(AttackKind::Melee);
                b.combo -= 1;
                b.pulse = true;
            } else if dist < 6.0 {
                intent.direction = to.normalize_or_zero();
            }
        }
    }
}

// ------------------------------------------------------------------------ puppets

/// Pose the stand-in bodies from the movement and combat state (Update).
fn animate_puppets(
    time: Res<Time>,
    mut puppets: Query<(&mut Puppet, &Mover, Option<&CombatState>)>,
    mut parts: Query<&mut Transform, With<PuppetPart>>,
) {
    let dt = time.real_delta_secs().max(0.0);
    let k = 1.0 - (-14.0 * dt).exp();
    for (mut p, m, cs) in &mut puppets {
        let defeated = cs.is_some_and(|c| c.defeated);
        let stagger = cs.is_some_and(|c| c.stagger_secs > 0.0);
        let crouch = if matches!(m.mode, LocomotionMode::Slide | LocomotionMode::Crouch) {
            1.0
        } else {
            0.0
        };
        let tilt = if stagger {
            Quat::from_rotation_x(0.35)
        } else {
            m.body_tilt()
        };
        let swing: f32 = match cs.and_then(|c| c.attack) {
            Some(a) => match a.phase {
                AttackPhase::DashIn => -0.6,
                AttackPhase::Windup => -1.0,
                AttackPhase::Strike => 1.0,
                AttackPhase::Recovery => 0.6,
            },
            None if cs.is_some_and(|c| c.blocking) => -0.3,
            None => 0.0,
        };
        p.tilt = p.tilt.slerp(tilt, k);
        p.crouch += (crouch - p.crouch) * k;
        p.fallen += ((if defeated { 1.0 } else { 0.0 }) - p.fallen) * k * 0.6;
        // Swings snap (they are fast); returning to rest is smoothed.
        p.swing = if swing.abs() > p.swing.abs() || swing.signum() != p.swing.signum() {
            swing
        } else {
            p.swing + (swing - p.swing) * k
        };
        if let Ok(mut rig) = parts.get_mut(p.rig) {
            let fall = Quat::from_rotation_x(-FRAC_PI_2 * p.fallen);
            rig.rotation = fall * p.tilt;
            rig.translation = Vec3::new(0.0, -0.35 * p.crouch - 0.55 * p.fallen, 0.0).as_dvec3();
            rig.scale = Vec3::new(1.0, 1.0 - 0.35 * p.crouch, 1.0);
        }
        if let Ok(mut sword) = parts.get_mut(p.sword) {
            // Rest: point forward-down. Windup: raised behind. Strike: swept across.
            let s = p.swing;
            let pitch = if s < 0.0 {
                -0.5 + s * 1.6
            } else {
                -0.5 + s * 0.3
            };
            let yaw = if s > 0.0 { s * 1.9 } else { 0.0 };
            sword.rotation = Quat::from_rotation_y(yaw) * Quat::from_rotation_x(pitch);
            sword.translation = Vec3::new(0.36 - 0.25 * s.max(0.0), 0.05, -0.1).as_dvec3();
        }
    }
}
