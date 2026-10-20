//! The movement showcase: [`spawn_demo`] builds it around a point.
//!
//! * **Parkour course** (+Z side): a runner loops sprint → slide under a bar → wall-run →
//!   wall-jump → wall-run → mantle onto a tower → jump and double jump off it. Afterimages
//!   trail every fast move, so a still frame shows the route. Round the back of the tower a
//!   second pilot climbs the caged ladder, steps off onto the roof, looks out over the course,
//!   then climbs back down a few rungs and slides the rest of the way.
//! * **Vault lane** (south of the runway): a pilot sprints down a lane and vaults a steel
//!   railing, a concrete barrier and a stack of equipment cases without breaking stride,
//!   a hand planted on each.
//! * **Ledge station** (between the two): a climber jumps at a block too high to climb
//!   straight onto, catches its top, shimmies along it and around the corner at its end,
//!   climbs up and walks off.
//! * **Grapple tower** (behind the course): a grappler sprints at a 9 m tower, jumps, fires
//!   its grapple hook at the top edge, is reeled up the wall and climbs over the top, then
//!   runs under the launch gantry on the roof, jumps into the zip line and rides it down
//!   26 m to a landing gate, letting go to drop through the gate onto its pad.
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
use engine_world::locomotion::{Climbable, LocomotionMode, MoveIntent};
use engine_world::physics::{CharacterController, Collider, PhysicsBackend, RigidBody, Velocity};
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
    /// The caged ladder up the tower's north face (+Z), climbed facing −Z: the climber's feet
    /// on its lowest rung (0.4 m up, a step off the runway) and on its top one (level with the
    /// roof), 0.4 m out from the face, and where it steps off onto the roof. Its rungs are 0.3
    /// m apart, 0.2 m out from the face.
    pub const LADDER_BOTTOM: [f64; 3] = [23.2, 0.416, 14.9];
    pub const LADDER_TOP: [f64; 3] = [23.2, TOWER_TOP + 0.016, 14.9];
    pub const LADDER_EXIT: [f64; 3] = [23.2, TOWER_TOP, 13.7];
    /// Where the ladder climber starts (feet), facing the ladder.
    pub const LADDER_START: [f64; 3] = [23.2, 0.1, 16.8];
    /// The ladder climber climbs down this far from the top, then slides, m.
    pub const LADDER_SLIDE_FROM: f32 = 1.5;
    /// The vault lane (south of the runway, west of the ring): a 26 m pad run along +X at
    /// z = 2, across it a railing, a concrete barrier and a stack of cases, each low and
    /// shallow enough to vault at a sprint.
    pub const VAULT_PAD: Block = Block {
        center: [-15.5, 0.05, 2.0],
        size: [26.0, 0.1, 5.0],
    };
    /// Where the vaulter starts (feet), facing +X, and where it stops running (x).
    pub const VAULT_START: [f64; 3] = [-27.0, 0.1, 2.0];
    pub const VAULT_END_X: f64 = -8.0;
    /// A steel railing, 1.1 m high, 4 m wide (its collider a solid panel).
    pub const VAULT_RAIL: Block = Block {
        center: [-22.0, 0.65, 2.0],
        size: [0.08, 1.1, 4.0],
    };
    /// A precast concrete barrier, 0.81 m high and 2.96 m long: its colliders step in with the
    /// drawn profile and stay inside it, the lower body (the first, from the ground), the
    /// sloped foot and the upright top, flush with the drawn top.
    pub const VAULT_BARRIER: [Block; 3] = [
        Block {
            center: [-16.0, 0.305, 2.0],
            size: [0.25, 0.41, 2.96],
        },
        Block {
            center: [-16.0, 0.175, 2.0],
            size: [0.5, 0.15, 2.96],
        },
        Block {
            center: [-16.0, 0.71, 2.0],
            size: [0.2, 0.4, 2.96],
        },
    ];
    /// Two equipment cases on a pallet, 1 m high and 1.2 m deep.
    pub const VAULT_CASES: Block = Block {
        center: [-10.0, 0.6, 2.0],
        size: [1.2, 1.0, 2.4],
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
    /// z = 2.0 (toward the course), shimmies toward +X and around the corner onto the +X end,
    /// climbs up there and walks off it.
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
    /// The zip line from the grapple tower's roof down to a landing gate on its own pad: its
    /// top anchor, under the launch gantry's crossbeam, and its bottom anchor, under the
    /// gate's.
    pub const ZIP_FROM: [f64; 3] = [10.4, GRAPPLE_TOP + 3.8, 31.0];
    pub const ZIP_TO: [f64; 3] = [33.0, 2.9, 21.6];
    /// The line's sag at mid-span, m.
    pub const ZIP_SAG: f32 = 0.45;
    /// Riders let go this far before the gate (and drop through it onto the pad), m.
    pub const ZIP_END_CLEAR: f32 = 4.0;
    /// From the line to each post of the launch gantry and of the landing gate, m.
    pub const ZIP_GANTRY_HALF: f64 = 1.1;
    pub const ZIP_GATE_HALF: f64 = 1.4;
    /// Tops of the gantry's posts (over the roof) and of the gate's (over the ground).
    pub const ZIP_GANTRY_TOP: f64 = ZIP_FROM[1] + 0.3;
    pub const ZIP_GATE_TOP: f64 = ZIP_TO[1] + 0.35;
    /// The landing pad around the bottom of the line.
    pub const ZIP_PAD: Block = Block {
        center: [33.75, 0.05, 20.75],
        size: [10.5, 0.1, 10.5],
    };
    /// The grappler's way onto the line from where it climbs over the roof's edge, in the
    /// line's frame (m along it from the top anchor, m across it): round the front of the
    /// gantry's post, then under the line, facing down it…
    pub const ZIP_APPROACH: [[f64; 2]; 2] = [[0.9, -1.7], [0.3, 0.0]];
    /// …and it jumps once this far along.
    pub const ZIP_TAKEOFF: f64 = 0.7;

    /// Along the zip line (from top to bottom) and across it, both level and unit.
    pub fn zip_axes() -> ([f64; 3], [f64; 3]) {
        let (dx, dz) = (ZIP_TO[0] - ZIP_FROM[0], ZIP_TO[2] - ZIP_FROM[2]);
        let l = (dx * dx + dz * dz).sqrt();
        ([dx / l, 0.0, dz / l], [-dz / l, 0.0, dx / l])
    }

    /// The posts of the launch gantry and of the landing gate (colliders): two each, either
    /// side of the line.
    pub fn zip_posts() -> [Block; 4] {
        let (_, across) = zip_axes();
        let post = |anchor: [f64; 3], half: f64, side: f64, base: f64, top: f64| Block {
            center: [
                anchor[0] + across[0] * half * side,
                (base + top) * 0.5,
                anchor[2] + across[2] * half * side,
            ],
            size: [0.2, (top - base) as f32, 0.2],
        };
        [
            post(ZIP_FROM, ZIP_GANTRY_HALF, 1.0, GRAPPLE_TOP, ZIP_GANTRY_TOP),
            post(ZIP_FROM, ZIP_GANTRY_HALF, -1.0, GRAPPLE_TOP, ZIP_GANTRY_TOP),
            post(ZIP_TO, ZIP_GATE_HALF, 1.0, 0.1, ZIP_GATE_TOP),
            post(ZIP_TO, ZIP_GATE_HALF, -1.0, 0.1, ZIP_GATE_TOP),
        ]
    }
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
    /// Loops the course tower's ladder: climb up onto the roof, look out, climb back down and
    /// slide the rest.
    LadderClimber {
        step: Rungs,
        secs: f32,
        #[serde(skip)]
        origin: DVec3,
    },
    /// Loops a ladder another module placed (`movement.ladder_guest`): climb it, step off at
    /// the top, look round, climb back down.
    LadderGuest {
        step: Rungs,
        secs: f32,
        #[serde(skip)]
        ladder: Option<Entity>,
    },
    /// Loops the vault lane: sprint over the railing, the barrier and the cases.
    Vaulter {
        step: Vaults,
        secs: f32,
        #[serde(skip)]
        origin: DVec3,
    },
    /// Loops the grapple tower: run, jump, hook the top edge, reel up, climb over, ride the zip
    /// line down.
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
    spawn_ladder(commands, origin);
    block(commands, origin, "Ledge station", layout::LEDGE_PAD);
    block(commands, origin, "Ledge block", layout::LEDGE_WALL);
    block(commands, origin, "Vault lane", layout::VAULT_PAD);
    block(commands, origin, "Vault railing", layout::VAULT_RAIL);
    for part in layout::VAULT_BARRIER {
        block(commands, origin, "Vault barrier", part);
    }
    block(commands, origin, "Vault cases", layout::VAULT_CASES);
    block(commands, origin, "Grapple pad", layout::GRAPPLE_PAD);
    block(commands, origin, "Grapple tower", layout::GRAPPLE_TOWER);
    spawn_zipline(commands, origin);
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
        "Ladder climber",
        origin + DVec3::from_array(layout::LADDER_START) + DVec3::Y * 0.9,
        Vec3::NEG_Z,
        Style::Pilot,
        MovementProfile::pilot(),
        Brain::LadderClimber {
            step: Rungs::Start,
            secs: 0.0,
            origin,
        },
    );

    spawn_character(
        commands,
        "Vaulter",
        origin + DVec3::from_array(layout::VAULT_START) + DVec3::Y * 0.9,
        Vec3::X,
        Style::Pilot,
        MovementProfile::pilot(),
        Brain::Vaulter {
            step: Vaults::Start,
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
            run_ladder_climber,
            run_ladder_guest,
            run_vaulter,
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
            crate::look::zipline::draw_ziplines,
            crate::look::zipline::drop_orphan_cables,
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
    /// Shimmy along the top toward +X, around the corner and on along the end (the stick held
    /// toward +X all the way).
    Shimmy,
    /// Hold toward the block: climb up onto it.
    ClimbUp,
    /// Walk off the far end and drop to the floor.
    WalkOff,
    /// Wait before looping.
    Rest,
}

/// Where the climber stops shimmying along the block's end and climbs, z relative to the
/// origin.
const CLIMB_AT_Z: f64 = 1.25;

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
                let on_end = m
                    .hang
                    .is_some_and(|h| h.corner.is_none() && h.normal.dot(Vec3::X) > 0.9);
                (on_end && rel.z <= CLIMB_AT_Z).then_some(Climb::ClimbUp)
            }
            Climb::ClimbUp => {
                intent.direction = m.hang.map_or(Vec3::NEG_X, |h| -h.normal);
                (m.grounded && rel.y > layout::LEDGE_TOP).then_some(Climb::WalkOff)
            }
            Climb::WalkOff => {
                intent.direction = Vec3::X * 0.8;
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

// ------------------------------------------------------------------------ the ladder climber

/// The caged ladder up the course tower's north face.
fn spawn_ladder(commands: &mut Commands, origin: DVec3) {
    let bottom = DVec3::from_array(layout::LADDER_BOTTOM);
    let rel = |p: [f64; 3]| (DVec3::from_array(p) - bottom).as_vec3();
    commands.spawn((
        Name::new("Tower ladder"),
        Transform::from_translation(origin + bottom),
        Climbable::ladder(Vec3::ZERO, rel(layout::LADDER_TOP), Vec3::NEG_Z)
            .with_half_width(0.23)
            .with_exit(rel(layout::LADDER_EXIT)),
    ));
}

/// The ladder climber's loop at the course tower.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Rungs {
    /// Stand at the start (teleported there) facing the ladder.
    #[default]
    Start,
    /// Walk to the ladder until the hands are on it.
    Walk,
    /// Climb to the top and step off onto the roof.
    Up,
    /// Turn round on the roof and look out over the course.
    Look,
    /// Walk back to the ladder: grabbed at the top, climbed down a few rungs…
    Down,
    /// …then slide the rest of the way.
    Slide,
    /// Step back from the ladder.
    Off,
    /// Wait before looping.
    Rest,
}

#[allow(clippy::type_complexity)]
fn run_ladder_climber(
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
        let Brain::LadderClimber { step, secs, origin } = &mut *brain else {
            continue;
        };
        *secs += real;
        let rel = t.translation - *origin;
        *intent = MoveIntent::default();
        let next = match *step {
            Rungs::Start => {
                if *secs < 0.02 {
                    t.translation =
                        *origin + DVec3::from_array(layout::LADDER_START) + DVec3::Y * 0.9;
                    t.rotation = Quat::IDENTITY;
                    m.reset_motion();
                    m.facing = Vec3::NEG_Z;
                    if let Some(mut i) = interp {
                        i.reset();
                    }
                }
                intent.look = Some(Vec3::NEG_Z);
                (*secs >= 0.8).then_some(Rungs::Walk)
            }
            Rungs::Walk => {
                intent.direction = Vec3::NEG_Z * 0.6;
                m.ladder.is_some().then_some(Rungs::Up)
            }
            Rungs::Up => {
                intent.direction = Vec3::NEG_Z;
                let on_roof = rel.y > layout::TOWER_TOP + 0.5;
                (m.grounded && on_roof && m.mode != LocomotionMode::Mantle).then_some(Rungs::Look)
            }
            Rungs::Look => {
                intent.look = Some(Vec3::Z);
                (*secs >= 1.6).then_some(Rungs::Down)
            }
            Rungs::Down => {
                intent.direction = Vec3::Z * 0.6;
                let top = (layout::LADDER_TOP[1] - layout::LADDER_BOTTOM[1]) as f32;
                m.ladder
                    .is_some_and(|l| l.climbed < top - layout::LADDER_SLIDE_FROM)
                    .then_some(Rungs::Slide)
            }
            Rungs::Slide => {
                intent.crouch = m.ladder.is_some();
                (m.ladder.is_none() && m.grounded).then_some(Rungs::Off)
            }
            Rungs::Off => {
                intent.direction = Vec3::Z * 0.5;
                intent.look = Some(Vec3::NEG_Z);
                (*secs >= 0.6).then_some(Rungs::Rest)
            }
            Rungs::Rest => (*secs >= 0.8).then_some(Rungs::Start),
        };
        // Stuck (knocked off, say): start over.
        if *secs > 6.0 {
            *step = Rungs::Start;
            *secs = 0.0;
        } else if let Some(n) = next {
            *step = n;
            *secs = 0.0;
        }
    }
}

/// Starts every ladder climber's loop over (for tour stops that time a capture).
pub(crate) fn restart_ladder_climbers(world: &mut World) -> usize {
    let mut q = world.query::<&mut Brain>();
    let mut n = 0;
    for mut b in q.iter_mut(world) {
        if let Brain::LadderClimber { step, secs, .. } = &mut *b {
            *step = Rungs::Start;
            *secs = 0.0;
            n += 1;
        }
    }
    n
}

// ------------------------------------------------------------------------ ladder guests

/// A ladder guest waits this long for a way up (a hatch to open) before starting over, s.
const GUEST_PATIENCE: f32 = 15.0;

#[allow(clippy::type_complexity)]
fn run_ladder_guest(
    time: Res<Time>,
    ladders: Query<(&Climbable, &Transform, &GlobalTransform, Has<ChildOf>)>,
    mut q: Query<
        (
            &mut Brain,
            &mut Transform,
            &mut Mover,
            &mut MoveIntent,
            Option<&mut TransformInterpolation>,
        ),
        Without<Climbable>,
    >,
) {
    let real = time.real_delta_secs();
    for (mut brain, mut t, mut m, mut intent, interp) in &mut q {
        let Brain::LadderGuest { step, secs, ladder } = &mut *brain else {
            continue;
        };
        *intent = MoveIntent::default();
        let Some(line) = ladder
            .and_then(|e| ladders.get(e).ok())
            .map(|(c, lt, g, parented)| crate::locomotion::climb_line(c, lt, g, parented))
        else {
            // The ladder is gone: stand still.
            continue;
        };
        *secs += real;
        let facing = line.facing;
        let len = (line.top - line.bottom).length() as f32;
        let feet = t.translation - DVec3::Y * 0.9;
        let next = match *step {
            Rungs::Start => {
                if *secs < 0.02 {
                    let at = line.bottom - (facing * 1.4).as_dvec3();
                    t.translation = at + DVec3::Y * 0.95;
                    t.rotation =
                        engine_core::transform::look_rotation(facing, Vec3::Y).unwrap_or_default();
                    m.reset_motion();
                    m.facing = facing;
                    if let Some(mut i) = interp {
                        i.reset();
                    }
                }
                intent.look = Some(facing);
                (*secs >= 0.8).then_some(Rungs::Walk)
            }
            Rungs::Walk => {
                intent.direction = facing * 0.6;
                m.ladder.is_some().then_some(Rungs::Up)
            }
            Rungs::Up => {
                intent.direction = facing;
                let on_top = feet.y > line.top.y - 0.3;
                (m.ladder.is_none() && m.grounded && on_top && m.mode != LocomotionMode::Mantle)
                    .then_some(Rungs::Look)
            }
            Rungs::Look => {
                intent.look = Some(-facing);
                (*secs >= 1.6).then_some(Rungs::Down)
            }
            Rungs::Down => {
                intent.direction = -facing * 0.6;
                let on = m.ladder.map(|l| l.climbed);
                if on.is_some_and(|c| c < len - layout::LADDER_SLIDE_FROM) {
                    Some(Rungs::Slide)
                } else {
                    (on.is_none() && m.grounded && feet.y < line.top.y - 1.0).then_some(Rungs::Off)
                }
            }
            Rungs::Slide => {
                intent.crouch = m.ladder.is_some();
                (m.ladder.is_none() && m.grounded).then_some(Rungs::Off)
            }
            Rungs::Off => {
                intent.direction = -facing * 0.5;
                intent.look = Some(facing);
                (*secs >= 0.6).then_some(Rungs::Rest)
            }
            Rungs::Rest => (*secs >= 0.8).then_some(Rungs::Start),
        };
        // Stuck (knocked off, a hatch that stays shut): start over.
        if *secs > GUEST_PATIENCE {
            *step = Rungs::Start;
            *secs = 0.0;
        } else if let Some(n) = next {
            *step = n;
            *secs = 0.0;
        }
    }
}

/// Sends a pilot up and down the ladder nearest `near` (world meters; else the active
/// camera): restarts the guest already on it, or spawns one. Returns the guest and the ladder.
pub(crate) fn spawn_ladder_guest(
    world: &mut World,
    near: Option<DVec3>,
) -> Result<(Entity, Entity), String> {
    let near = near
        .or_else(|| crate::play::active_camera(world).map(|(_, g)| g.translation()))
        .ok_or("no point given and no active camera")?;
    let mut ladders = world.query::<(
        Entity,
        &Climbable,
        &Transform,
        &GlobalTransform,
        Has<ChildOf>,
    )>();
    let (ladder, line) = ladders
        .iter(world)
        .map(|(e, c, t, g, parented)| (e, crate::locomotion::climb_line(c, t, g, parented)))
        .min_by(|a, b| a.1.distance(near).total_cmp(&b.1.distance(near)))
        .ok_or("no ladder (Climbable) in this world")?;
    let mut brains = world.query::<(Entity, &mut Brain)>();
    for (e, mut b) in brains.iter_mut(world) {
        if let Brain::LadderGuest {
            step,
            secs,
            ladder: Some(on),
        } = &mut *b
            && *on == ladder
        {
            *step = Rungs::Start;
            *secs = 0.0;
            return Ok((e, ladder));
        }
    }
    let at = line.bottom - (line.facing * 1.4).as_dvec3() + DVec3::Y * 0.95;
    let mut queue = engine_core::ecs::world::CommandQueue::default();
    let guest = {
        let mut commands = Commands::new(&mut queue, world);
        spawn_character(
            &mut commands,
            "Ladder guest",
            at,
            line.facing,
            Style::Pilot,
            MovementProfile::pilot(),
            Brain::LadderGuest {
                step: Rungs::Start,
                secs: 0.0,
                ladder: Some(ladder),
            },
        )
    };
    queue.apply(world);
    Ok((guest, ladder))
}

// ------------------------------------------------------------------------ the vaulter

/// The vaulter's loop along the vault lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Vaults {
    /// Stand at the start (teleported there) facing down the lane.
    #[default]
    Start,
    /// Sprint down the lane: the railing, the barrier and the cases are vaulted on the way.
    Run,
    /// Pull up at the end of the lane.
    Stop,
}

#[allow(clippy::type_complexity)]
fn run_vaulter(
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
        let Brain::Vaulter { step, secs, origin } = &mut *brain else {
            continue;
        };
        *secs += real;
        let rel = t.translation - *origin;
        *intent = MoveIntent::default();
        let next = match *step {
            Vaults::Start => {
                if *secs < 0.02 {
                    t.translation =
                        *origin + DVec3::from_array(layout::VAULT_START) + DVec3::Y * 0.9;
                    t.rotation = Quat::from_rotation_y(-FRAC_PI_2);
                    m.reset_motion();
                    m.facing = Vec3::X;
                    if let Some(mut i) = interp {
                        i.reset();
                    }
                }
                intent.look = Some(Vec3::X);
                (*secs >= 0.6).then_some(Vaults::Run)
            }
            Vaults::Run => {
                // Down the middle of the lane.
                let back = (layout::VAULT_START[2] - rel.z) as f32;
                intent.direction = Vec3::new(1.0, 0.0, back.clamp(-0.3, 0.3)).normalize();
                intent.sprint = true;
                (rel.x > layout::VAULT_END_X && m.vault.is_none()).then_some(Vaults::Stop)
            }
            Vaults::Stop => {
                intent.look = Some(Vec3::X);
                (*secs >= 1.2).then_some(Vaults::Start)
            }
        };
        // Stuck: start over.
        if *secs > 8.0 {
            *step = Vaults::Start;
            *secs = 0.0;
        } else if let Some(n) = next {
            *step = n;
            *secs = 0.0;
        }
    }
}

/// Starts every vaulter's loop over (for tour stops that time a capture).
pub(crate) fn restart_vaulters(world: &mut World) -> usize {
    let mut q = world.query::<&mut Brain>();
    let mut n = 0;
    for mut b in q.iter_mut(world) {
        if let Brain::Vaulter { step, secs, .. } = &mut *b {
            *step = Vaults::Start;
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
    /// Walk round the front of the zip line's gantry…
    ToGantry,
    /// …and under the line, facing down it.
    ToZip,
    /// Run a step and jump into the line.
    ZipRun,
    /// Ride the line down to the landing gate.
    Zip,
    /// Let go at the gate: drop onto the pad.
    Land,
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
        let (along, across) = layout::zip_axes();
        let (along, across) = (DVec3::from_array(along), DVec3::from_array(across));
        let down = along.as_vec3();
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
                    .then_some(GrappleLoop::ToGantry)
            }
            GrappleLoop::ToGantry | GrappleLoop::ToZip => {
                let last = *step == GrappleLoop::ToZip;
                let [a, c] = layout::ZIP_APPROACH[last as usize];
                let to = DVec3::from_array(layout::ZIP_FROM) + along * a + across * c - rel;
                let to = Vec3::new(to.x as f32, 0.0, to.z as f32);
                intent.direction =
                    to.normalize_or_zero() * if to.length() > 0.5 { 0.6 } else { 0.3 };
                intent.look = Some(if !last || to.length() > 0.5 {
                    to.normalize_or(down)
                } else {
                    down
                });
                match last {
                    false => (to.length() < 0.4).then_some(GrappleLoop::ToZip),
                    true => (to.length() < 0.25 && m.facing.dot(down) > 0.95)
                        .then_some(GrappleLoop::ZipRun),
                }
            }
            GrappleLoop::ZipRun => {
                intent.direction = down;
                let past = (rel - DVec3::from_array(layout::ZIP_FROM)).dot(along);
                intent.jump = past > layout::ZIP_TAKEOFF;
                intent.jump.then_some(GrappleLoop::Zip)
            }
            // Caught, rode it down and let go (or missed it and fell).
            GrappleLoop::Zip => (*secs > 1.0 && m.zip.is_none()).then_some(GrappleLoop::Land),
            GrappleLoop::Land => m.grounded.then_some(GrappleLoop::Rest),
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

/// Shuttles a kinematic body back and forth along a straight track: out from `from` to `to`, a
/// stand there, back, and a stand at `from`, easing in and out of each run, on world time.
/// With a physics backend it drives the body by its `Velocity` (the backend moves it and
/// writes the `Transform` back, so what stands on it is carried); with only the stand-in
/// physics it sets the `Transform` and keeps the `Velocity` true to how it moved.
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
    /// Stand still for a step after a teleport, so a physics backend places the body before
    /// it moves by velocity again.
    #[serde(skip)]
    pub hold: bool,
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

fn drive_shuttles(
    time: Res<Time>,
    backend: Option<Res<PhysicsBackend>>,
    mut q: Query<(&mut Shuttle, &mut Transform, &mut Velocity)>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    for (mut s, mut t, mut v) in &mut q {
        if std::mem::take(&mut s.hold) {
            v.linear = Vec3::ZERO;
            continue;
        }
        s.secs = (s.secs + dt).rem_euclid(s.cycle_secs());
        let to = s.position();
        // From where it is, so it never drifts off the track. A backend moves it by its
        // velocity; without one it is placed, and the velocity still says how fast it moves
        // (characters judge a wall-run along it by that).
        v.linear = ((to - t.translation) / dt as f64).as_vec3();
        if backend.is_none() {
            t.translation = to;
        }
    }
}

/// The zip line from the grapple tower's roof to the landing gate, the posts of the gantry and
/// the gate it hangs from, and the landing pad.
fn spawn_zipline(commands: &mut Commands, origin: DVec3) {
    let from = origin + DVec3::from_array(layout::ZIP_FROM);
    let to = DVec3::from_array(layout::ZIP_TO) - DVec3::from_array(layout::ZIP_FROM);
    commands.spawn((
        Name::new("Zip line"),
        Transform::from_translation(from),
        crate::zipline::Zipline {
            to: to.as_vec3(),
            sag: layout::ZIP_SAG,
            end_clear: layout::ZIP_END_CLEAR,
            exit_hop: 0.5,
        },
    ));
    for post in layout::zip_posts() {
        block(commands, origin, "Zip line post", post);
    }
    block(commands, origin, "Zip line pad", layout::ZIP_PAD);
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
            Velocity::default(),
            TransformInterpolation::default(),
            Shuttle {
                from,
                to,
                travel_secs: layout::FREIGHT_TRAVEL_SECS,
                dwell_secs: layout::FREIGHT_DWELL_SECS,
                // Standing at the start, about to pull out.
                secs: 2.0 * layout::FREIGHT_TRAVEL_SECS + 1.5 * layout::FREIGHT_DWELL_SECS,
                hold: false,
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
        Option<&mut Velocity>,
        Option<&mut TransformInterpolation>,
    )>();
    for (mut s, mut t, v, interp) in cars.iter_mut(world) {
        s.secs = s.cycle_secs() - 0.5;
        s.hold = true;
        t.translation = s.position();
        if let Some(mut v) = v {
            v.linear = Vec3::ZERO;
        }
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
        zip: m.zip.map(|z| {
            let grip = Vec3::Y * (cc.height * (0.5 + crate::zipline::GRIP_ABOVE));
            (grip, inv * z.heading)
        }),
        ladder: m.ladder.map(|l| rig::LadderAnim {
            climbed: l.climbed,
            speed: l.speed,
            sliding: l.sliding,
        }),
        vault: m.vault.map(|v| {
            let left = m.up.cross(v.forward).normalize_or_zero();
            let hand = v.plant + (left * crate::vault::HAND_LEFT).as_dvec3();
            rig::VaultAnim {
                plant: inv * (hand - feet).as_vec3(),
                along: (feet - v.plant).as_vec3().dot(v.forward),
                depth: v.depth,
            }
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
