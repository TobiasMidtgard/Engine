//! The movement showcase: [`spawn_demo`] builds it around a point.
//!
//! * **Parkour course** (+Z side): a runner loops sprint → slide under a bar → wall-run →
//!   wall-jump → wall-run → mantle onto a tower → jump and double jump off it. Afterimages
//!   trail every fast move, so a still frame shows the route.
//! * **Ledge station** (between the two): a climber jumps at a block too high to climb
//!   straight onto, catches its top, shimmies along it, climbs up and walks off the far end.
//! * **Grapple tower** (behind the course): a grappler sprints at a 9 m tower, jumps, fires
//!   its grapple hook at the top edge, is reeled up the wall, climbs over the top, runs across
//!   and drops off the back.
//! * **Freight station** (beside the grapple tower): a freight car shuttles along a short
//!   track past a loading platform. As it pulls out, a rider sprints along the platform,
//!   leaps onto the car's side and wall-runs along it, climbs over onto its roof, rides it to
//!   the end of the track and drops back onto the platform.
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
//! The characters are armored sword fighters ([`look::fighter`]) posed procedurally from the
//! movement and combat state on their own clocks ([`look::rig`]): rigid parts on a skeleton,
//! with IK for the arms and legs. The bots that drive them write `MoveIntent` exactly like
//! player input or NPC AI would. Only one demo per world.

use std::f32::consts::FRAC_PI_2;

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_core::time::LocalTimeScale;
use engine_render::{Camera, MeshRenderer, StandardMaterial};
use engine_world::animation::AnimationParams;
use engine_world::locomotion::{LocomotionMode, MoveIntent};
use engine_world::physics::{CharacterController, Collider, RigidBody};
use engine_world::{AttackEvent, AttackKind};

use crate::afterimage::Afterimages;
use crate::combat::{AttackPhase, CombatState, Fighter};
use crate::look::fighter::{self, Style};
use crate::look::rig::{self, AnimInput, Animator, AttackAnim, Bone, Xf};
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
    /// The ledge station's floor: a slab between the course and the ring.
    pub const LEDGE_PAD: Block = Block {
        center: [17.0, 0.05, 2.0],
        size: [12.0, 0.1, 7.0],
    };
    /// Top of the ledge block: too high to climb straight onto, in reach of a jump and a grab.
    pub const LEDGE_TOP: f64 = 3.7;
    /// The ledge block on the station's floor. The climber catches its top from the face at
    /// z = 2.0 (toward the course), shimmies toward +X, climbs up and walks off the +X end.
    pub const LEDGE_WALL: Block = Block {
        center: [17.0, (LEDGE_TOP + 0.1) * 0.5, 1.2],
        size: [6.0, (LEDGE_TOP - 0.1) as f32, 1.6],
    };
    /// Where the climber starts (feet), facing the ledge block.
    pub const CLIMB_START: [f64; 3] = [15.0, 0.1, 4.6];
    /// The grapple tower's floor: a slab behind the course (+Z of the runway).
    pub const GRAPPLE_PAD: Block = Block {
        center: [8.0, 0.05, 29.0],
        size: [10.0, 0.1, 22.0],
    };
    /// Top of the grapple tower: far too high to climb without the hook.
    pub const GRAPPLE_TOP: f64 = 9.0;
    /// The grapple tower. Its front face (z = 28) faces the course.
    pub const GRAPPLE_TOWER: Block = Block {
        center: [8.0, GRAPPLE_TOP * 0.5, 31.0],
        size: [8.0, GRAPPLE_TOP as f32, 6.0],
    };
    /// The grapple target on the tower's front face, just under its top edge.
    pub const GRAPPLE_TARGET: [f64; 3] = [8.0, GRAPPLE_TOP - 0.35, 28.0];
    /// Where the grappler starts (feet), facing the tower.
    pub const GRAPPLE_START: [f64; 3] = [8.0, 0.1, 19.0];
    /// The grappler jumps (and fires once airborne) as it crosses this z.
    pub const GRAPPLE_TAKEOFF_Z: f64 = 21.0;
    /// The freight station's loading platform, beside the track (−Z side). Its +Z edge is at
    /// z = 27.3, 1.2 m short of the car's side.
    pub const FREIGHT_PLATFORM: Block = Block {
        center: [-14.0, 0.5, 25.4],
        size: [24.0, 1.0, 3.8],
    };
    /// The track's center line.
    pub const FREIGHT_TRACK_Z: f64 = 30.0;
    /// Where the car's center stands at each end of the track.
    pub const FREIGHT_FROM_X: f64 = -20.0;
    pub const FREIGHT_TO_X: f64 = -7.5;
    /// The freight car's collider (full size): a box from just above the rails to its roof.
    pub const FREIGHT_CAR_SIZE: [f32; 3] = [12.0, 3.6, 3.0];
    /// Top of the car's roof.
    pub const FREIGHT_ROOF: f64 = 4.0;
    /// Seconds rolling from one end of the track to the other, and standing at each end.
    pub const FREIGHT_TRAVEL_SECS: f32 = 5.0;
    pub const FREIGHT_DWELL_SECS: f32 = 3.5;
    /// Where the rider starts (feet), on the platform beside the waiting car.
    pub const RIDE_START: [f64; 3] = [-24.0, 1.0, 25.4];
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
    /// Loops the ledge station: catch the top, shimmy, climb up, walk off the end.
    Climber {
        step: Climb,
        secs: f32,
        #[serde(skip)]
        origin: DVec3,
    },
    /// Loops the grapple tower: run, jump, hook the top edge, reel up, climb over.
    Grappler {
        step: GrappleLoop,
        secs: f32,
        #[serde(skip)]
        origin: DVec3,
    },
    /// Loops the freight station: leap onto the departing car, wall-run along it, climb
    /// aboard, ride it, drop off.
    Rider {
        step: Ride,
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

/// A procedurally animated fighter: the bone entities it poses every frame (children of the
/// character; see [`look::rig`]) and its animation state.
#[derive(Component, Debug, Clone)]
pub struct Puppet {
    bones: [Entity; Bone::ANIMATED],
    anim: Animator,
    was_defeated: bool,
    /// The left hand as last posed (character space, feet at the capsule's bottom).
    hand_l: Xf,
}

impl Puppet {
    pub fn style(&self) -> Style {
        self.anim.style
    }

    /// The left hand as last posed, relative to the character's transform.
    pub fn hand_l(&self) -> Xf {
        self.hand_l
    }
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

pub(crate) fn spawn_character(
    commands: &mut Commands,
    name: &str,
    pos: DVec3,
    facing: Vec3,
    style: Style,
    profile: MovementProfile,
    extra: impl Bundle,
) -> Entity {
    let rotation = engine_core::transform::look_rotation(facing, Vec3::Y).unwrap_or_default();
    let controller = CharacterController::default();
    let id = commands
        .spawn((
            Name::new(name.to_string()),
            Transform::from_translation(pos).with_rotation(rotation),
            profile,
            controller,
            Afterimages::colored(style.trail()).lasting(0.8),
            extra,
        ))
        .id();
    // The body: posed bones under the character with the feet at the capsule's bottom.
    let body = fighter::spawn_body(commands, id, style, -controller.height * 0.5);
    let mut bones = [Entity::PLACEHOLDER; Bone::ANIMATED];
    bones.copy_from_slice(&body.bones[..Bone::ANIMATED]);
    for &bone in &bones {
        commands.entity(bone).insert(PuppetPart);
    }
    commands.entity(id).insert(Puppet {
        bones,
        anim: Animator::new(style),
        was_defeated: false,
        hand_l: Xf::IDENTITY,
    });
    id
}

/// Build the movement showcase around `origin` (ground level). See the module docs.
pub fn spawn_demo(commands: &mut Commands, origin: DVec3) {
    // P (the `pilot` action) takes control of a pilot here.
    commands.insert_resource(crate::play::PlayToggle);
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
    block(commands, origin, "Ledge station", layout::LEDGE_PAD);
    block(commands, origin, "Ledge block", layout::LEDGE_WALL);
    block(commands, origin, "Grapple pad", layout::GRAPPLE_PAD);
    block(commands, origin, "Grapple tower", layout::GRAPPLE_TOWER);
    block(
        commands,
        origin,
        "Freight platform",
        layout::FREIGHT_PLATFORM,
    );
    spawn_freight_car(commands, origin);
    // The pilot training facility drawn around those boxes.
    crate::look::course::dress_course(commands, origin);

    spawn_character(
        commands,
        "Runner",
        origin + DVec3::from_array(layout::RUN_START) + DVec3::Y * 0.9,
        Vec3::X,
        Style::Pilot,
        MovementProfile::pilot(),
        Brain::Runner {
            step: 0,
            secs: 0.0,
            origin,
        },
    );

    spawn_character(
        commands,
        "Climber",
        origin + DVec3::from_array(layout::CLIMB_START) + DVec3::Y * 0.9,
        Vec3::NEG_Z,
        Style::Pilot,
        MovementProfile::pilot(),
        Brain::Climber {
            step: Climb::Start,
            secs: 0.0,
            origin,
        },
    );

    spawn_character(
        commands,
        "Grappler",
        origin + DVec3::from_array(layout::GRAPPLE_START) + DVec3::Y * 0.9,
        Vec3::Z,
        Style::Pilot,
        MovementProfile::pilot(),
        Brain::Grappler {
            step: GrappleLoop::Start,
            secs: 0.0,
            origin,
        },
    );

    spawn_character(
        commands,
        "Rider",
        origin + DVec3::from_array(layout::RIDE_START) + DVec3::Y * 0.9,
        Vec3::X,
        Style::Pilot,
        MovementProfile::pilot(),
        Brain::Rider {
            step: Ride::Start,
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
        Style::Duelist,
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
        Style::Elite,
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
            Style::Grunt,
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
    .register_component::<Shuttle>(
        "Shuttles a body back and forth along a straight track (the movement demo's freight \
         car): seconds into its cycle, travel and dwell seconds.",
    )
    .add_systems(
        FixedUpdate,
        (
            drive_shuttles,
            direct_duel,
            run_runner,
            run_climber,
            run_grappler,
            run_rider,
            think_duelists,
            think_attackers,
        )
            .chain()
            .in_set(SimSet::Input)
            .before(crate::MovementSet::Intent)
            .before(crate::fallback::publish),
    )
    .add_systems(
        Update,
        (
            animate_puppets,
            crate::look::rope::draw_ropes,
            crate::look::rope::drop_orphan_ropes,
        )
            .chain(),
    );
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

// ------------------------------------------------------------------------ the climber

/// The climber's loop at the ledge station.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Climb {
    /// Stand at the start (teleported there) facing the block.
    #[default]
    Start,
    /// Run at the block and jump a stride from it: too high to mantle, so the hands catch it.
    Approach,
    /// Hang for a moment.
    Hang,
    /// Shimmy along the top toward +X.
    Shimmy,
    /// Hold toward the block: climb up onto it.
    ClimbUp,
    /// Walk off the far end and drop to the floor.
    WalkOff,
    /// Wait before looping.
    Rest,
}

/// Where the climber stops shimmying and climbs, x relative to the origin.
const CLIMB_AT_X: f64 = 18.4;

#[allow(clippy::type_complexity)]
fn run_climber(
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
        let Brain::Climber { step, secs, origin } = &mut *brain else {
            continue;
        };
        *secs += real;
        let rel = t.translation - *origin;
        let face = layout::LEDGE_WALL.center[2] + layout::LEDGE_WALL.size[2] as f64 * 0.5;
        *intent = MoveIntent::default();
        let next = match *step {
            Climb::Start => {
                if *secs < 0.02 {
                    t.translation =
                        *origin + DVec3::from_array(layout::CLIMB_START) + DVec3::Y * 0.9;
                    t.rotation = Quat::IDENTITY;
                    m.reset_motion();
                    m.facing = Vec3::NEG_Z;
                    if let Some(mut i) = interp {
                        i.reset();
                    }
                }
                intent.look = Some(Vec3::NEG_Z);
                (*secs >= 0.8).then_some(Climb::Approach)
            }
            Climb::Approach => {
                intent.direction = Vec3::NEG_Z;
                intent.jump = m.grounded && rel.z < face + 1.35;
                (m.mode == LocomotionMode::Climb).then_some(Climb::Hang)
            }
            Climb::Hang => (*secs >= 0.7).then_some(Climb::Shimmy),
            Climb::Shimmy => {
                intent.direction = Vec3::X;
                (rel.x >= CLIMB_AT_X).then_some(Climb::ClimbUp)
            }
            Climb::ClimbUp => {
                intent.direction = Vec3::NEG_Z;
                (m.grounded && rel.y > layout::LEDGE_TOP).then_some(Climb::WalkOff)
            }
            Climb::WalkOff => {
                intent.direction = Vec3::X * 0.5;
                (*secs > 0.3 && m.grounded && rel.y < 1.5).then_some(Climb::Rest)
            }
            Climb::Rest => (*secs >= 1.0).then_some(Climb::Start),
        };
        // Stuck (knocked off, say): start over.
        if *secs > 6.0 {
            *step = Climb::Start;
            *secs = 0.0;
        } else if let Some(n) = next {
            *step = n;
            *secs = 0.0;
        }
    }
}

/// Restart every climber's loop.
pub(crate) fn restart_climbers(world: &mut World) -> usize {
    let mut q = world.query::<&mut Brain>();
    let mut n = 0;
    for mut b in q.iter_mut(world) {
        if let Brain::Climber { step, secs, .. } = &mut *b {
            *step = Climb::Start;
            *secs = 0.0;
            n += 1;
        }
    }
    n
}

// ------------------------------------------------------------------------ the grappler

/// The grappler's loop at the grapple tower.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum GrappleLoop {
    /// Stand at the start (teleported there) facing the tower.
    #[default]
    Start,
    /// Sprint at the tower.
    Run,
    /// Jump; fire the hook at the top edge on the way up.
    Jump,
    /// Reeled up the tower, then over the edge.
    Reel,
    /// Run across the top and off the back.
    Cross,
    /// Falling to the ground behind the tower.
    Drop,
    /// Wait before looping.
    Rest,
}

#[allow(clippy::type_complexity)]
fn run_grappler(
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
        let Brain::Grappler { step, secs, origin } = &mut *brain else {
            continue;
        };
        *secs += real;
        let rel = t.translation - *origin;
        *intent = MoveIntent::default();
        let next = match *step {
            GrappleLoop::Start => {
                if *secs < 0.02 {
                    t.translation =
                        *origin + DVec3::from_array(layout::GRAPPLE_START) + DVec3::Y * 0.9;
                    t.rotation = Quat::from_rotation_y(std::f32::consts::PI);
                    m.reset_motion();
                    m.facing = Vec3::Z;
                    if let Some(mut i) = interp {
                        i.reset();
                    }
                }
                intent.look = Some(Vec3::Z);
                (*secs >= 1.0).then_some(GrappleLoop::Run)
            }
            GrappleLoop::Run => {
                intent.direction = Vec3::Z;
                intent.sprint = true;
                (rel.z >= layout::GRAPPLE_TAKEOFF_Z).then_some(GrappleLoop::Jump)
            }
            GrappleLoop::Jump => {
                intent.direction = Vec3::Z;
                intent.jump = *secs < 0.1;
                let fire = *secs >= 0.2 && !m.grounded;
                if fire {
                    m.pending_grapple = Some(*origin + DVec3::from_array(layout::GRAPPLE_TARGET));
                }
                fire.then_some(GrappleLoop::Reel)
            }
            GrappleLoop::Reel => {
                intent.direction = Vec3::Z;
                let on_top = rel.y > layout::GRAPPLE_TOP + 0.5;
                (m.grounded && on_top && m.mode != LocomotionMode::Mantle)
                    .then_some(GrappleLoop::Cross)
            }
            GrappleLoop::Cross => {
                // A jog over the top, so the drop off the back lands on the pad.
                intent.direction = Vec3::Z * 0.5;
                (!m.grounded && rel.z > layout::GRAPPLE_TOWER.center[2] + 2.5)
                    .then_some(GrappleLoop::Drop)
            }
            GrappleLoop::Drop => {
                intent.direction = Vec3::Z * 0.3;
                (m.grounded && rel.y < 2.0).then_some(GrappleLoop::Rest)
            }
            GrappleLoop::Rest => (*secs >= 1.0).then_some(GrappleLoop::Start),
        };
        // Stuck (missed the edge, say): start over.
        if *secs > 6.0 {
            *step = GrappleLoop::Start;
            *secs = 0.0;
        } else if let Some(n) = next {
            *step = n;
            *secs = 0.0;
        }
    }
}

/// Starts every grappler's loop over (for tour stops that time a capture).
pub(crate) fn restart_grapplers(world: &mut World) -> usize {
    let mut q = world.query::<&mut Brain>();
    let mut n = 0;
    for mut b in q.iter_mut(world) {
        if let Brain::Grappler { step, secs, .. } = &mut *b {
            *step = GrappleLoop::Start;
            *secs = 0.0;
            n += 1;
        }
    }
    n
}

// ------------------------------------------------------------------------ the freight station

/// Shuttles a body back and forth along a straight track: out from `from` to `to`, a stand
/// there, back, and a stand at `from`, easing in and out of each run. It moves the body's
/// `Transform` on world time, so a kinematic body is driven like any scripted platform.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Shuttle {
    #[serde(skip)]
    pub from: DVec3,
    #[serde(skip)]
    pub to: DVec3,
    /// Seconds a run from one end to the other takes.
    pub travel_secs: f32,
    /// Seconds standing at each end.
    pub dwell_secs: f32,
    /// Seconds into the cycle (world time).
    pub secs: f32,
}

/// Where a [`Shuttle`] is in its cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Leg {
    /// Running from `from` to `to`.
    Out,
    /// Standing at `to`.
    AtEnd,
    /// Running back.
    Back,
    /// Standing at `from` (the cycle starts with the run out).
    AtStart,
}

impl Shuttle {
    pub fn cycle_secs(&self) -> f32 {
        2.0 * (self.travel_secs + self.dwell_secs)
    }

    /// The leg it is on and the seconds into it.
    pub fn leg(&self) -> (Leg, f32) {
        let (run, stand) = (self.travel_secs, self.dwell_secs);
        let t = self.secs.rem_euclid(self.cycle_secs());
        if t < run {
            (Leg::Out, t)
        } else if t < run + stand {
            (Leg::AtEnd, t - run)
        } else if t < 2.0 * run + stand {
            (Leg::Back, t - run - stand)
        } else {
            (Leg::AtStart, t - 2.0 * run - stand)
        }
    }

    /// Where it is now.
    pub fn position(&self) -> DVec3 {
        let ease = |t: f32| 0.5 - 0.5 * (std::f32::consts::PI * t / self.travel_secs).cos();
        match self.leg() {
            (Leg::Out, t) => self.from.lerp(self.to, ease(t) as f64),
            (Leg::AtEnd, _) => self.to,
            (Leg::Back, t) => self.to.lerp(self.from, ease(t) as f64),
            (Leg::AtStart, _) => self.from,
        }
    }
}

fn drive_shuttles(time: Res<Time>, mut q: Query<(&mut Shuttle, &mut Transform)>) {
    let dt = time.delta_secs();
    for (mut s, mut t) in &mut q {
        s.secs = (s.secs + dt).rem_euclid(s.cycle_secs());
        t.translation = s.position();
    }
}

/// The freight car: a kinematic box shuttling along the track, dressed as a box car.
fn spawn_freight_car(commands: &mut Commands, origin: DVec3) {
    let y = layout::FREIGHT_ROOF - layout::FREIGHT_CAR_SIZE[1] as f64 * 0.5;
    let from = at(origin, layout::FREIGHT_FROM_X, y, layout::FREIGHT_TRACK_Z);
    let to = at(origin, layout::FREIGHT_TO_X, y, layout::FREIGHT_TRACK_Z);
    let car = commands
        .spawn((
            Name::new("Freight car"),
            Transform::from_translation(from),
            RigidBody::kinematic(),
            Collider::cuboid(Vec3::from(layout::FREIGHT_CAR_SIZE) * 0.5),
            TransformInterpolation::default(),
            Shuttle {
                from,
                to,
                travel_secs: layout::FREIGHT_TRAVEL_SECS,
                dwell_secs: layout::FREIGHT_DWELL_SECS,
                // Standing at the start, about to pull out.
                secs: 2.0 * layout::FREIGHT_TRAVEL_SECS + 1.5 * layout::FREIGHT_DWELL_SECS,
            },
        ))
        .id();
    crate::look::course::dress_freight_car(commands, car);
}

/// The rider's loop at the freight station.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Ride {
    /// Stand at the start (teleported there) on the platform.
    #[default]
    Start,
    /// Wait for the car to pull out.
    Wait,
    /// Sprint along the platform, catching up with the car.
    Sprint,
    /// Cut toward the platform's edge, at the car's side.
    Cut,
    /// Jump and steer at the car's side.
    Leap,
    /// Run along the car's side.
    WallRun,
    /// Steer into the car near its roof: climb over onto it.
    Climb,
    /// Ride the car to the end of the track.
    Aboard,
    /// Step off its side onto the platform.
    Off,
    /// Wait before looping.
    Rest,
}

#[allow(clippy::type_complexity)]
fn run_rider(
    time: Res<Time>,
    cars: Query<(&Shuttle, &Transform), Without<Brain>>,
    mut q: Query<(
        &mut Brain,
        &mut Transform,
        &mut Mover,
        &mut MoveIntent,
        Option<&mut TransformInterpolation>,
    )>,
) {
    let real = time.real_delta_secs();
    let Some((car, car_t)) = cars.iter().next() else {
        return;
    };
    let (leg, leg_secs) = car.leg();
    for (mut brain, mut t, mut m, mut intent, interp) in &mut q {
        let Brain::Rider { step, secs, origin } = &mut *brain else {
            continue;
        };
        *secs += real;
        let rel = t.translation - *origin;
        let car_x = (car_t.translation - *origin).x;
        *intent = MoveIntent::default();
        let toward_car = |along: f32, into: f32| Vec3::new(along, 0.0, into).normalize();
        let platform_edge =
            layout::FREIGHT_PLATFORM.center[2] + layout::FREIGHT_PLATFORM.size[2] as f64 * 0.5;
        let next = match *step {
            Ride::Start => {
                if *secs < 0.02 {
                    t.translation =
                        *origin + DVec3::from_array(layout::RIDE_START) + DVec3::Y * 0.9;
                    t.rotation = Quat::from_rotation_y(-FRAC_PI_2);
                    m.reset_motion();
                    m.facing = Vec3::X;
                    if let Some(mut i) = interp {
                        i.reset();
                    }
                }
                intent.look = Some(Vec3::X);
                (*secs >= 0.3).then_some(Ride::Wait)
            }
            Ride::Wait => {
                intent.look = Some(Vec3::X);
                (leg == Leg::Out && leg_secs >= 0.6).then_some(Ride::Sprint)
            }
            Ride::Sprint => {
                intent.direction = Vec3::X;
                intent.sprint = true;
                (rel.x >= car_x - 4.0).then_some(Ride::Cut)
            }
            Ride::Cut => {
                intent.direction = toward_car(1.0, 0.45);
                intent.sprint = true;
                (rel.z >= platform_edge - 0.35).then_some(Ride::Leap)
            }
            Ride::Leap => {
                intent.direction = toward_car(0.35, 1.0);
                intent.sprint = true;
                intent.jump = *secs < 0.1;
                (m.mode == LocomotionMode::WallRun).then_some(Ride::WallRun)
            }
            Ride::WallRun => {
                intent.direction = Vec3::X;
                (m.mode != LocomotionMode::WallRun || m.mode_secs >= 0.15).then_some(Ride::Climb)
            }
            Ride::Climb => {
                intent.direction = toward_car(0.45, 1.0);
                let on_roof = rel.y > layout::FREIGHT_ROOF + 0.7;
                (m.grounded && on_roof && m.mode != LocomotionMode::Mantle).then_some(Ride::Aboard)
            }
            Ride::Aboard => {
                intent.look = Some(Vec3::X);
                (leg == Leg::AtEnd && leg_secs >= 0.4).then_some(Ride::Off)
            }
            Ride::Off => {
                intent.direction = Vec3::NEG_Z * 0.6;
                (m.grounded && rel.y < layout::FREIGHT_ROOF - 1.0).then_some(Ride::Rest)
            }
            Ride::Rest => (*secs >= 1.0).then_some(Ride::Start),
        };
        // Stuck (missed the car, say): start over. Waiting for the car and riding it take as
        // long as the car does.
        let patient = matches!(*step, Ride::Wait | Ride::Aboard);
        if !patient && *secs > 6.0 {
            *step = Ride::Start;
            *secs = 0.0;
        } else if let Some(n) = next {
            *step = n;
            *secs = 0.0;
        }
    }
}

/// Starts every rider's loop over, with the freight car about to pull out (for tour stops
/// that time a capture).
pub(crate) fn restart_riders(world: &mut World) -> usize {
    let mut n = 0;
    let mut q = world.query::<&mut Brain>();
    for mut b in q.iter_mut(world) {
        if let Brain::Rider { step, secs, .. } = &mut *b {
            *step = Ride::Start;
            *secs = 0.0;
            n += 1;
        }
    }
    let mut cars = world.query::<(
        &mut Shuttle,
        &mut Transform,
        Option<&mut TransformInterpolation>,
    )>();
    for (mut s, mut t, interp) in cars.iter_mut(world) {
        s.secs = s.cycle_secs() - 0.5;
        t.translation = s.position();
        if let Some(mut i) = interp {
            i.reset();
        }
    }
    n
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
                glow(0x3fd0ff, 3000.0)
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

/// Animate the fighters from their movement and combat state, on their own clocks (Update).
#[allow(clippy::type_complexity)]
fn animate_puppets(
    time: Res<Time>,
    mut puppets: Query<(
        &mut Puppet,
        &Transform,
        &Mover,
        &CharacterController,
        Option<&CombatState>,
        Option<&Fighter>,
        Option<&AnimationParams>,
        Option<&LocalTimeScale>,
    )>,
    mut bones: Query<&mut Transform, (With<PuppetPart>, Without<Puppet>)>,
) {
    for (mut p, t, m, cc, cs, f, params, local) in &mut puppets {
        let defeated = cs.is_some_and(|c| c.defeated);
        if p.was_defeated && !defeated {
            // Revived: stand straight up instead of getting up through the blend.
            p.anim.reset();
        }
        p.was_defeated = defeated;
        let input = anim_input(time.delta_for(local), t, m, cc, cs, f, params);
        let pose = p.anim.update(&input);
        let solved = rig::solve(&pose);
        let lift = -cc.height * 0.5;
        for (i, &e) in p.bones.iter().enumerate() {
            if let Ok(mut bt) = bones.get_mut(e) {
                *bt = fighter::bone_transform(solved[i], lift);
            }
        }
        let hand = solved[Bone::HandL.index()];
        p.hand_l = Xf::new(hand.pos + Vec3::Y * lift, hand.rot);
    }
}

/// What the animation needs to know, in character space (feet at the origin, facing −Z).
fn anim_input(
    dt: f32,
    t: &Transform,
    m: &Mover,
    cc: &CharacterController,
    cs: Option<&CombatState>,
    f: Option<&Fighter>,
    params: Option<&AnimationParams>,
) -> AnimInput {
    let inv = t.rotation.inverse();
    let feet = t.translation - (t.rotation * Vec3::Y * (cc.height * 0.5)).as_dvec3();
    let attack = cs.and_then(|c| c.attack).map(|a| {
        let dash = a.kind == AttackKind::Dash;
        let total = match (a.phase, f) {
            (AttackPhase::Windup, Some(f)) if dash => f.dash_windup_secs,
            (AttackPhase::Windup, Some(f)) => f.windup_secs,
            (AttackPhase::Strike, Some(f)) => f.strike_secs,
            (AttackPhase::Recovery, Some(f)) => f.recovery_secs,
            _ => 0.0,
        };
        let progress = if total > 0.0 {
            (1.0 - a.secs_left / total).clamp(0.0, 1.0)
        } else {
            0.0
        };
        AttackAnim {
            dash,
            phase: a.phase,
            progress,
            secs: a.secs,
        }
    });
    AnimInput {
        dt,
        mode: m.mode,
        velocity: inv * m.velocity,
        grounded: m.grounded,
        air_jumps_left: m.air_jumps_left,
        wall: m.wall.map(|w| {
            let gap = (t.translation - w.point).as_vec3().dot(w.normal).abs();
            (inv * w.normal, gap)
        }),
        ledge: m
            .mantle
            .map(|mt| (inv * (mt.top - feet).as_vec3(), mt.rising))
            .or_else(|| m.hang.map(|h| (inv * (h.top - feet).as_vec3(), true))),
        grapple: m.grapple.map(|g| {
            let from = crate::grapple::shoulders(t.translation, m.up, cc.height);
            let hook = crate::grapple::hook_position(&g, from);
            (inv * (hook - feet).as_vec3(), g.attached)
        }),
        dash_strike: m.dash.is_some_and(|d| d.strike),
        attack,
        blocking: cs.is_some_and(|c| c.blocking),
        stagger: cs.is_some_and(|c| c.stagger_secs > 0.0),
        hits_taken: cs.map_or(0, |c| c.hits_taken),
        defeated: cs.is_some_and(|c| c.defeated),
        fighter: f.is_some(),
        effort: params.map_or(0.2, |p| p.get("effort")),
    }
}
