//! Animal bodies: red deer, wolves, wild boar, rabbits and starlings built from the sculpted
//! pieces in
//! [`crate::art::animals`] (cheap primitives without the renderer) and posed every step
//! from what the animal is doing and what its body did the step before ([`Motion`]).
//!
//! Feet are planted: a hoof or paw on the ground keeps its spot while the body moves over it,
//! and each leg is solved with inverse kinematics from the shoulder (riding on the shoulder
//! blade) or hip to the toe, swinging out sideways where a turn needs it, so swinging legs fold
//! at the carpus or hock and stance legs straighten as they reach. The feet follow a footfall
//! pattern per gait, blended as the gait changes: deer walk in lateral sequence, trot and
//! gallop (rotary), and bound off when bolting from a stand; wolves walk, trot, lope and
//! gallop; boars walk, trot and gallop (transverse) on short legs; rabbits hop and
//! half-bound, the hind feet landing ahead of where the fore feet were. Strides lengthen and quicken with speed, and a foot pushed further than its leg
//! reaches steps early. Standing, feet left out of place step back under the body one at a
//! time.
//!
//! The body rides on the feet: it sinks into each stance in the running gaits and flies a
//! ballistic arc whenever no foot is down, pitches over the front or hind pair carrying the
//! weight, rears back as it sets off and pitches forward as it stops, then settles back, and
//! leans into turns. Deer, wolves and boars bend their backs: the torso is a fore half and a
//! croup
//! hinged at the loin, and through the gallop, in step with the feet, the croup tucks under as
//! the back arches in the gathered flight (hind feet swinging up past the fore feet) and
//! lifts as it hollows in the extended one. Heads and necks steady themselves against the bob
//! and nod with the walk; a galloping wolf carries its head low and level. Ears swivel toward
//! noises and threats and flick, tails swish at flies, a deer's tail flags up when it is
//! alarmed or bolting, breathing quickens after a run, deer graze with tearing pulls and look
//! up to chew and watch, and an alert animal throws its head up toward what alarmed it and
//! freezes before it scans. Whatever it turns to look at, the head goes round first (the eyes
//! are part of the head) and the neck follows.
//!
//! Wolves stalk with head and body low and rush from the stalk crouched through the first
//! bounds; a hunting wolf keeps its head on its prey, reaches for it with the jaws open as it
//! closes, and on a bite thrusts in at full gape, snaps shut, grips, shakes and tugs, then
//! lets go. A boar roots with its snout in the soil, shoving and sweeping it, the jaw
//! working, lifts it now and then to scent the air, champs when alarmed, runs with its tail
//! up, and charges head down to hook its tusks up and sideways into what it strikes. Hurt but
//! not killed, an animal flinches: it jolts away from the blow, hunched and ducking, head
//! jerking away, ears pinned flat and tail clamped, and staggers a step or two before it runs.
//! Animals lie down with folded legs. Killed, the legs give: the forequarters first
//! (pitching onto the chest and knees on the run, the feet skidding on under the body), then
//! the hindquarters; it goes over onto its side, away from the blow when that came from one
//! side, lands, rocks back off the ground, the limp legs and head flopping after the body,
//! and settles. A dead starling tumbles out of the air and lands the same way.
//!
//! Starlings beat their wings in a cycle (an extended downstroke, a folded upstroke with the
//! hand swept back), fold them for short bounds between bursts in level flight, glide on
//! spread wings when sinking, bank into turns with the head held level, push off with their
//! legs and sweep their wings wide to take off, and flare to land (body up, wings spread
//! forward, tail fanned and down, legs reaching), folding the wings away once down. On the
//! ground they hop and peck.
//!
//! Everything a pose remembers is in the animal's own frame and on its own clock, never in
//! world points, so re-anchoring the world doesn't disturb it; a jump in heading or time
//! starts it afresh.

use std::f32::consts::FRAC_PI_2;

use engine_core::prelude::*;
use engine_render::{MeshRenderer, Shape, StandardMaterial};
#[cfg(test)]
use engine_world::locomotion::LocomotionState;

use super::*;
use crate::art::ArtAssets;
use crate::art::animals::{
    self as art, BIRD, BOAR, BOAR_SCALE, DEER, PIGLET_SCALE, Piece, QuadRig, RABBIT, STAG_SCALE,
    WOLF,
};
use crate::wildlife::{Animal, AnimalMind, AnimalState, BirdFlight, Species};

// `Part::Beast` indices.
/// Middle leg segments (forearm, gaskin) of legs 0..4.
const LEG_MID: u8 = 0;
/// Lower leg segments (cannon, metapodial) of legs 0..4.
const LEG_LOW: u8 = 4;
/// Feet (pastern, paw) of legs 0..4.
const LEG_FOOT: u8 = 8;
/// Hooves of legs 0..4.
const HOOF: u8 = 12;
/// Eyes and nose (or beak).
const GLOSS: u8 = 16;
const NECK2: u8 = 17;
const TAIL2: u8 = 18;
/// A bird's tail spread in flight.
const TAIL_FAN: u8 = 19;
/// A bird's wings folded on its back (left, right).
const WING_FOLDED: u8 = 20;
/// The hind half of a deer's, wolf's or boar's torso.
const CROUP: u8 = 22;
/// A boar's lower tusks.
const TUSKS: u8 = 23;
/// A boar's crests (nape, withers).
const MANE: u8 = 24;
/// A boar's upper tusks.
const UPPER_TUSKS: u8 = 26;
/// How far a boar's crests slide up out of the coat when raised (rig m).
const MANE_LIFT: f32 = 0.045;

fn part_of(piece: Piece) -> Part {
    match piece {
        Piece::Body => Part::Body,
        Piece::Croup => Part::Beast(CROUP),
        Piece::Neck => Part::Neck,
        Piece::Neck2 => Part::Beast(NECK2),
        Piece::Head => Part::Head,
        Piece::Gloss => Part::Beast(GLOSS),
        Piece::Jaw => Part::Snout,
        Piece::Ear(s) => Part::Ear(s),
        Piece::Tail => Part::Tail,
        Piece::Tail2 => Part::Beast(TAIL2),
        Piece::Antlers => Part::Antler(0),
        Piece::Tusks => Part::Beast(TUSKS),
        Piece::Mane(s) => Part::Beast(MANE + s),
        Piece::UpperTusks => Part::Beast(UPPER_TUSKS),
        Piece::Leg(k, 0) => Part::Leg(k),
        Piece::Leg(k, 1) => Part::Beast(LEG_MID + k),
        Piece::Leg(k, 2) => Part::Beast(LEG_LOW + k),
        Piece::Leg(k, _) => Part::Beast(LEG_FOOT + k),
        Piece::Hoof(k) => Part::Beast(HOOF + k),
        Piece::Wing(s, 0) => Part::Wing(s),
        Piece::Wing(s, 1) => Part::Wing(2 + s),
        Piece::Wing(s, _) => Part::Beast(WING_FOLDED + s),
        Piece::TailFan => Part::Beast(TAIL_FAN),
    }
}

fn body_scale(species: Species, male: bool, young: bool) -> f32 {
    match species {
        Species::Deer if male => STAG_SCALE,
        Species::Boar if young => PIGLET_SCALE,
        Species::Boar if male => BOAR_SCALE,
        _ => 1.0,
    }
}

/// The parts of an animal's body.
pub(super) fn parts(
    stand_in: &StandIn,
    animal: Option<&Animal>,
    species: Species,
    art: &mut ArtAssets,
) -> Vec<(Part, MeshRenderer, Vec3)> {
    let male = animal.is_some_and(|a| a.male);
    let young = animal.is_some_and(|a| a.young);
    let scale = Vec3::splat(body_scale(species, male, young));
    let tint = stand_in.color.map(|c| {
        let want = StandardMaterial::hex(c).base_color;
        let have = StandardMaterial::hex(species_color(species)).base_color;
        Vec3::new(
            want[0] / have[0].max(0.01),
            want[1] / have[1].max(0.01),
            want[2] / have[2].max(0.01),
        )
        .min(Vec3::splat(2.0))
    });
    if let Some(pieces) = art::pieces(species, male, young, tint, art) {
        return pieces
            .into_iter()
            .map(|(piece, mesh, material)| {
                let mut r = MeshRenderer::new(mesh, material);
                r.cast_shadows = piece != Piece::Gloss;
                (part_of(piece), r, scale)
            })
            .collect();
    }
    // Without the renderer nothing is drawn: keep the same parts as cheap primitives.
    let color = mat(stand_in.color.unwrap_or_else(|| species_color(species)));
    let blob = Shape::Sphere {
        radius: match species {
            Species::Deer => 0.08,
            Species::Wolf => 0.06,
            Species::Rabbit => 0.03,
            Species::Bird => 0.015,
            Species::Boar => 0.07,
        },
        segments: 6,
    };
    art::piece_names(species, male, young)
        .into_iter()
        .map(|piece| (part_of(piece), MeshRenderer::new(blob, color), scale))
        .collect()
}

// ======================================================================= input and output

/// What an animal's pose is made from this step.
pub(super) struct Input {
    pub species: Species,
    pub male: bool,
    pub young: bool,
    pub state: AnimalState,
    /// When the state began and the time now (NPC clock, s).
    pub since: f64,
    pub now: f64,
    /// The mover's gait phase (rad): where a pose with no past starts its cycle.
    pub phase: f32,
    /// Where the animal is and which way it faces (its transform), and its velocity (m/s).
    pub at: DVec3,
    pub facing: Quat,
    pub vel: Vec3,
    pub crouch: bool,
    pub biting: bool,
    /// Per-animal variety, 0..1.
    pub vary: f32,
    pub senses: Senses,
    pub bird: Option<BirdFlight>,
}

/// What the animal has noticed, as its pose shows it (ears, head, tail).
#[derive(Debug, Clone, Copy)]
pub(super) struct Senses {
    /// What it watches or runs from, and how dangerous it is (≥ 1 inside its flight distance).
    pub threat: Option<DVec3>,
    pub danger: f32,
    /// Seconds since it last sensed real danger.
    pub alarmed: f32,
    /// A noise heard this step.
    pub heard: Option<DVec3>,
    /// 0..1: low after a hard run (it breathes hard).
    pub stamina: f32,
    /// Wolves: where the animal it hunts is.
    pub prey: Option<DVec3>,
    /// What last hurt it (killed, what killed it).
    pub hurt: Option<DVec3>,
    /// Its last wound: how long ago (s), and where the blow came from.
    pub wound: Option<(f32, DVec3)>,
}

impl Default for Senses {
    fn default() -> Self {
        Self {
            threat: None,
            danger: 0.0,
            alarmed: 1e3,
            heard: None,
            stamina: 1.0,
            prey: None,
            hurt: None,
            wound: None,
        }
    }
}

impl Senses {
    pub fn of(m: &AnimalMind, now: f64) -> Self {
        Self {
            threat: m.threat_pos,
            danger: m.danger,
            alarmed: (now - m.last_danger).clamp(0.0, 1e3) as f32,
            heard: m.heard.map(|(p, _)| p),
            stamina: m.stamina,
            prey: None,
            hurt: m.hurt_from,
            wound: m
                .wounded
                .map(|(at, from, _)| ((now - at).max(0.0) as f32, from)),
        }
    }
}

const SLOTS: usize = 43;

fn slot(part: Part) -> Option<usize> {
    Some(match part {
        Part::Body => 0,
        Part::Neck => 1,
        Part::Head => 2,
        Part::Snout => 3,
        Part::Tail => 4,
        Part::Ear(s) if s < 2 => 5 + s as usize,
        Part::Antler(_) => 7,
        Part::Leg(k) if k < 4 => 8 + k as usize,
        Part::Wing(w) if w < 4 => 12 + w as usize,
        Part::Beast(b) if b <= UPPER_TUSKS => 16 + b as usize,
        _ => return None,
    })
}

/// Every piece of one animal posed (relative to its owner).
#[derive(Clone, Copy)]
pub(super) struct Pose([Placed; SLOTS]);

impl Pose {
    fn new() -> Self {
        Self([HIDDEN; SLOTS])
    }

    fn set(&mut self, piece: Piece, p: Placed) {
        if let Some(i) = slot(part_of(piece)) {
            self.0[i] = p;
        }
    }

    pub(super) fn place(&self, part: Part) -> Placed {
        slot(part).map_or(HIDDEN, |i| self.0[i])
    }
}

/// Pose an animal for this step, carrying its motion on.
pub(super) fn animal(i: &Input, m: &mut Motion) -> Pose {
    let scale = body_scale(i.species, i.male, i.young);
    let k = tick(i, m, scale);
    let mut out = Pose::new();
    match i.species {
        Species::Deer => quad(&DEER, &DEER_BUILD, Beast::Deer, i, m, &k, &mut out),
        Species::Wolf => quad(&WOLF, &WOLF_BUILD, Beast::Wolf, i, m, &k, &mut out),
        Species::Boar => {
            let build = if i.young { &PIGLET_BUILD } else { &BOAR_BUILD };
            quad(&BOAR, build, Beast::Boar, i, m, &k, &mut out)
        }
        Species::Rabbit => rabbit(i, m, &k, &mut out),
        Species::Bird => bird(i, m, &k, &mut out),
    }
    for p in &mut out.0 {
        p.pos *= scale;
    }
    out
}

/// One piece of a pose with no past, from the old stateless inputs (tests).
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(super) fn animal_pose(
    species: Species,
    part: Part,
    state: AnimalState,
    since: f64,
    now: f64,
    phase: f32,
    speed: f32,
    loco: &LocomotionState,
    biting: bool,
    flap: f32,
    male: bool,
) -> Placed {
    let vel = if speed > 0.0 {
        Vec3::NEG_Z * speed
    } else {
        loco.velocity
    };
    let input = Input {
        species,
        male,
        young: false,
        state,
        since,
        now,
        phase,
        at: DVec3::ZERO,
        facing: Quat::IDENTITY,
        vel,
        crouch: loco.mode == engine_world::locomotion::LocomotionMode::Crouch,
        biting,
        vary: 0.37,
        senses: Senses::default(),
        bird: (species == Species::Bird).then_some(BirdFlight {
            velocity: loco.velocity,
            flap,
            ..BirdFlight::default()
        }),
    };
    animal(&input, &mut Motion::default()).place(part)
}

// ======================================================================= shared helpers

fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn wrap_pi(a: f32) -> f32 {
    (a + PI).rem_euclid(TAU) - PI
}

/// Direction at angle `a` in the YZ plane (from +Y toward +Z).
fn dir(a: f32) -> Vec2 {
    Vec2::new(a.cos(), a.sin())
}

/// Angle of a YZ direction.
fn angle(d: Vec2) -> f32 {
    d.y.atan2(d.x)
}

fn yz(p: Vec3) -> Vec2 {
    Vec2::new(p.y, p.z)
}

/// A ground point (owner frame x, z) turned `a` about the owner's vertical axis.
fn turn(p: Vec2, a: f32) -> Vec2 {
    let q = Quat::from_rotation_y(a) * Vec3::new(p.x, 0.0, p.y);
    Vec2::new(q.x, q.z)
}

/// A side for a killed animal to fall onto, by a number 0..1: +1 lands it on its left side
/// (right side up).
fn fall_side(seed: f32) -> f32 {
    if seed > 0.5 { 1.0 } else { -1.0 }
}

/// A short twitch every few seconds: 0 most of the time, up to 1.
fn twitch(now: f32, every: f32, seed: f32) -> f32 {
    let t = (now / every + seed * 7.3).fract() * every;
    smooth(0.0, 0.08, t) * (1.0 - smooth(0.15, 0.35, t))
}

/// Slow wandering value about −1..1.
fn drift(now: f32, rate: f32, seed: f32) -> f32 {
    let x = now * rate + seed * 17.0;
    (x.sin() * 0.6 + (x * 2.3 + 1.7).sin() * 0.3 + (x * 4.1 + 0.3).sin() * 0.1).clamp(-1.0, 1.0)
}

/// A value that holds still for a while, then moves to another: −1..1, new every `every` s.
fn glance(now: f32, every: f32, seed: f32) -> f32 {
    let n = (now / every + seed * 13.1).floor();
    ((n * 12.9898 + seed * 78.233).sin() * 43_758.547).fract() * 2.0 - 1.0
}

/// How a keyed value moves on from its key to the next.
#[derive(Debug, Clone, Copy)]
enum Ease {
    /// Speeding up (falling).
    In,
    /// Slowing down (rising off a bounce).
    Out,
    Both,
}

/// A keyed track at `t` s: `(time, value, how it gets there)`, held before the first key and
/// after the last.
fn track(t: f32, keys: &[(f32, f32, Ease)]) -> f32 {
    for w in keys.windows(2) {
        let ((ta, a, _), (tb, b, ease)) = (w[0], w[1]);
        if t < tb {
            let x = ((t - ta) / (tb - ta)).clamp(0.0, 1.0);
            let e = match ease {
                Ease::In => x * x,
                Ease::Out => x * (2.0 - x),
                Ease::Both => x * x * (3.0 - 2.0 * x),
            };
            return lerp(a, b, e);
        }
    }
    keys.last().map_or(0.0, |k| k.1)
}

/// Ease `x` toward `to` with time constant `secs`.
fn ease_to(x: &mut f32, to: f32, secs: f32, dt: f32) {
    *x += (to - *x) * (1.0 - (-dt / secs.max(1e-3)).exp());
}

/// A damped spring (follow-through, weight shifts).
#[derive(Debug, Clone, Copy, Default)]
struct Spring {
    x: f32,
    v: f32,
}

impl Spring {
    fn step(&mut self, to: f32, hz: f32, damping: f32, dt: f32) {
        let w = TAU * hz;
        let n = (dt / 0.008).ceil().clamp(1.0, 64.0);
        let h = dt / n;
        for _ in 0..n as usize {
            let a = w * w * (to - self.x) - 2.0 * damping * w * self.v;
            self.v += a * h;
            self.x += self.v * h;
        }
    }
}

/// A rigid frame: rest coordinates relative to `origin` map to `pos + rot * (p - origin)`.
#[derive(Debug, Clone, Copy)]
struct Frame {
    origin: Vec3,
    pos: Vec3,
    rot: Quat,
}

impl Frame {
    fn point(&self, rest: Vec3) -> Vec3 {
        self.pos + self.rot * (rest - self.origin)
    }

    /// Rest coordinates of an owner-space point.
    fn rest(&self, p: Vec3) -> Vec3 {
        self.origin + self.rot.inverse() * (p - self.pos)
    }

    /// The frame of a child joint at rest position `pivot`, turned by `local`.
    fn child(&self, pivot: Vec3, local: Quat) -> Frame {
        Frame {
            origin: pivot,
            pos: self.point(pivot),
            rot: self.rot * local,
        }
    }

    fn placed(&self) -> Placed {
        at(self.pos, self.rot)
    }
}

fn flip(p: Vec3) -> Vec3 {
    Vec3::new(-p.x, p.y, p.z)
}

// ======================================================================= motion

/// One foot's memory.
#[derive(Debug, Clone, Copy, Default)]
struct Foot {
    /// Where it stands on the ground (owner frame x and z, rig meters), or, in the air, where
    /// it was under the body when it lifted.
    at: Vec2,
    /// In the air: since when (NPC clock), for how long (s) and how high it lifts (rig m).
    air: Option<(f64, f32, f32)>,
    /// Coming down: the spot it comes down on (owner frame, carried with the ground like a
    /// planted foot, so the foot drops straight onto it whatever the body does meanwhile).
    aim: Option<Vec2>,
    /// The gait lap in which it last lifted (a foot lifts once a lap).
    lap: i64,
    /// When it last came down (NPC clock).
    down: f64,
}

/// A foot as posed this step.
#[derive(Debug, Clone, Copy, Default)]
struct Plant {
    /// Where it is over the ground (owner frame x and z, rig m) and how high.
    at: Vec2,
    lift: f32,
    /// On the ground: how far through its time down (0..1); in the air: through its swing.
    u: f32,
    down: bool,
}

/// What the feet do this step.
#[derive(Debug, Clone, Copy)]
struct Pattern {
    /// When each foot (left fore, right fore, left hind, right hind) touches down in the
    /// cycle, and the share of the cycle it is down.
    offsets: [f32; 4],
    duty: [f32; 4],
    /// Cycles per second while stepping.
    freq: f32,
    stepping: bool,
    /// How high each foot lifts (rig m).
    lift: [f32; 4],
    /// Where each foot stands under the body (owner frame x and z, rig m).
    home: [Vec2; 4],
    /// How far a planted foot may get from its place before it must step (rig m).
    reach: [f32; 4],
    /// Standing, a foot further than this out of place steps back (rig m), in this long (s).
    tidy: f32,
    tidy_secs: f32,
}

/// What an animal's pose remembers from one step to the next: its gait clock, where its feet
/// stand, how fast it was going and turning, the weight it threw, where it last heard
/// something, its ears and tail, and a bird's wing beat. Purely visual, kept in the animal's
/// own frame and on its clock.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Motion {
    posed: bool,
    /// When it was last posed (NPC clock) and its heading then.
    last: f64,
    yaw: f32,
    /// Velocity (owner frame x and z, m/s), smoothed acceleration (m/s²) and turn rate (rad/s,
    /// positive turning left).
    vel: Vec2,
    accel: Vec2,
    spin: f32,
    /// The state it was in last step.
    state: Option<AnimalState>,
    /// Gait cycles run (the fraction is where in the cycle), the gait in use (0 walk, 1
    /// trot, ...), and whether the feet were stepping.
    cycles: f64,
    gait: f32,
    stepping: bool,
    feet: [Foot; 4],
    /// No foot down: since when and for how long until the first lands (s).
    flight: Option<(f64, f32)>,
    /// Pitch (rad) and surge (rig m) from speeding up and slowing down; the tail's swing.
    lean: Spring,
    surge: Spring,
    tail: Spring,
    /// Breathing cycle (0..1).
    breath: f32,
    /// It bolted from a stand (or a wolf rushed from its stalk): the first bounds are leaps.
    startle: bool,
    /// A wolf's last bite: when its jaws snapped shut, and whether it was biting last step.
    bit: Option<f64>,
    biting: bool,
    /// A dead bird: when it came to rest on the ground, and the roll and pitch it came down
    /// with (rad); falling, the roll and pitch of its tumble.
    downed: Option<(f64, f32, f32)>,
    tumble: (f32, f32),
    /// Killed: the side it falls onto (+1 its left side, right side up; 0 alive), and how
    /// fast it was going (rig m/s).
    side: f32,
    struck: f32,
    /// The last noise heard: when, and where (bearing from ahead, rad, right positive).
    noise: Option<(f64, f32)>,
    /// The last wound: when, and where the blow came from (bearing, as for a noise).
    flinch: Option<(f64, f32)>,
    /// Head and neck turned toward what it watches (rad, left positive), and the ears'
    /// swivels (rad, forward positive).
    look: f32,
    ears: [f32; 2],
    /// The head turned on ahead of the neck toward what it looks at (rad, left positive).
    gaze: f32,
    /// A bird's wing beat (rad), beats since it last folded its wings, the bound under way
    /// (when it began and how long it lasts), how much it glides, flares and banks.
    wing: f32,
    beats: u32,
    bound: Option<(f64, f32)>,
    glide: f32,
    flare: f32,
    bank: f32,
    /// The head, neck, ears and tail as last posed, and a change of state fading out of them
    /// (when it began, and the look it left).
    was: Look,
    fade: Option<(f64, Look)>,
    /// How far it crouches (0..1); a rabbit sitting up, grazing and lying low (0..1).
    crouch: f32,
    blend: [f32; 3],
}

/// This step's timing and motion in the animal's own frame.
struct Tick {
    now: f64,
    dt: f32,
    turned: f32,
    fresh: bool,
    /// Seconds in the current state, a clock for idle motion (wrapped), variety 0..1 (per
    /// state) and per animal.
    t: f32,
    clock: f32,
    seed: f32,
    vary: f32,
    /// Velocity (owner frame x and z, rig m/s) and horizontal speed (rig m/s).
    v: Vec2,
    speed: f32,
    /// Bearing (rad from ahead, right positive) of what it watches, if anything, and how far
    /// above or below it is (rad, up positive).
    threat: Option<f32>,
    threat_up: f32,
    /// Wolves: bearing (rad from ahead, right positive) and distance (m) of the prey.
    prey: Option<(f32, f32)>,
    /// Killed: the side it falls onto (+1 its left side, right side up).
    side: f32,
    /// Hurt (alive): seconds since, and the bearing of the blow.
    flinch: Option<(f32, f32)>,
}

fn tick(i: &Input, m: &mut Motion, scale: f32) -> Tick {
    let fwd = i.facing * Vec3::NEG_Z;
    let yaw = (-fwd.x).atan2(-fwd.z);
    let mut dt = (i.now - m.last) as f32;
    let mut turned = wrap_pi(yaw - m.yaw);
    let fresh = !m.posed || !(0.0..=0.5).contains(&dt) || turned.abs() > 1.2;
    if fresh {
        *m = Motion::default();
        dt = 0.0;
        turned = 0.0;
    }
    let local = Quat::from_rotation_y(yaw).inverse() * i.vel;
    let v = Vec2::new(local.x, local.z);
    if dt > 0.0 {
        let a = ((v - m.vel) / dt).clamp_length_max(30.0);
        m.accel = m.accel.lerp(a, (dt * 8.0).min(1.0));
        m.spin += (turned / dt - m.spin) * (dt * 12.0).min(1.0);
    }
    m.vel = v;
    m.last = i.now;
    m.yaw = yaw;
    m.posed = true;
    let bearing = |p: DVec3| {
        let d = Quat::from_rotation_y(yaw).inverse() * (p - i.at).as_vec3();
        d.x.atan2(-d.z)
    };
    let seed = ((i.since * 0.618_034).fract() as f32 + 1.0).fract();
    // Killed, it falls away from the blow that killed it when that came from one side, else
    // the way it leans into a turn, else the way its stride throws it; one first seen dead
    // lies as chance has it.
    if i.state != AnimalState::Dead {
        m.side = 0.0;
    } else if m.side == 0.0 {
        let blow = i.senses.hurt.map_or(0.0, |p| bearing(p).sin());
        let lean = m.spin * v.length();
        m.side = if blow.abs() > 0.2 {
            blow.signum()
        } else if fresh {
            fall_side(seed)
        } else if lean.abs() > 2.0 {
            lean.signum()
        } else {
            fall_side((m.cycles.rem_euclid(1.0) as f32 + m.wing / TAU).rem_euclid(1.0))
        };
        m.struck = v.length() / scale;
    }
    if let Some(p) = i.heard {
        m.noise = Some((i.now, bearing(p)));
    }
    if let Some((_, b)) = &mut m.noise {
        *b = wrap_pi(*b + turned);
    }
    // A new wound: where the blow came from, kept as it turns.
    if let Some((ago, from)) = i.wound {
        let at = i.now - f64::from(ago);
        if m.flinch.is_none_or(|(t, _)| (t - at).abs() > 1e-3) {
            m.flinch = Some((at, bearing(from)));
        }
    }
    if let Some((_, b)) = &mut m.flinch {
        *b = wrap_pi(*b + turned);
    }
    let flinch = m
        .flinch
        .map(|(at, b)| ((i.now - at) as f32, b))
        .filter(|&(t, _)| (0.0..2.0).contains(&t) && i.state != AnimalState::Dead);
    Tick {
        now: i.now,
        dt,
        turned,
        fresh,
        t: (i.now - i.since).max(0.0) as f32,
        clock: (i.now % 10_000.0) as f32,
        seed,
        vary: i.vary,
        v: v / scale,
        speed: v.length() / scale,
        threat: i.senses.threat.map(bearing),
        threat_up: i.senses.threat.map_or(0.0, |p| {
            let d = (p - i.at).as_vec3();
            let flat = Vec2::new(d.x, d.z).length().max(1.0);
            (d.y / flat).atan().clamp(-0.5, 0.5)
        }),
        prey: i.senses.prey.map(|p| {
            let d = (p - i.at).as_vec3();
            (bearing(p), Vec2::new(d.x, d.z).length())
        }),
        side: m.side,
        flinch,
    }
}

impl std::ops::Deref for Input {
    type Target = Senses;
    fn deref(&self) -> &Senses {
        &self.senses
    }
}

impl Motion {
    /// Where foot `k` should come down to be under its place halfway through its time down,
    /// `left` s from now, the body going on as it goes now.
    fn landing(p: &Pattern, k: usize, v: Vec2, spin: f32, left: f32) -> Vec2 {
        let half = if p.stepping {
            p.duty[k] / (2.0 * p.freq.max(1e-3))
        } else {
            0.0
        };
        let tau = left + half;
        turn(p.home[k] + v * tau, spin * tau)
    }

    /// Set the feet down as a steady gait has them at this point of its cycle.
    fn reset_feet(&mut self, now: f64, v: Vec2, p: &Pattern) {
        for k in 0..4 {
            let c = self.cycles - f64::from(p.offsets[k]);
            let (t, lap) = (c.rem_euclid(1.0) as f32, c.floor() as i64);
            let duty = p.duty[k];
            let freq = p.freq.max(1e-3);
            let half = duty / (2.0 * freq);
            self.feet[k] = if p.stepping && t >= duty {
                let u = (t - duty) / (1.0 - duty).max(1e-3);
                let dur = (1.0 - duty) / freq;
                Foot {
                    at: p.home[k] - v * half,
                    air: Some((now - f64::from(u * dur), dur, p.lift[k])),
                    lap,
                    down: now - 1.0,
                    aim: None,
                }
            } else if p.stepping {
                Foot {
                    at: p.home[k] + v * (half - t / freq),
                    air: None,
                    lap: lap - 1,
                    down: now - f64::from(t / freq),
                    aim: None,
                }
            } else {
                Foot {
                    at: p.home[k],
                    air: None,
                    lap: lap - 1,
                    down: now - 1.0,
                    aim: None,
                }
            };
        }
        self.stepping = p.stepping;
        self.flight = None;
    }

    /// Move the feet on `dt` s, the body having moved at `v` (owner frame, rig m/s) and
    /// turned by `turned` (rad). A foot on the ground stays where it is on the ground; one in
    /// the air swings to where it will stand. A foot lifts when the gait's cycle comes round
    /// to it, when the body has carried it as far as the leg reaches, or, standing, when it
    /// is out of place and the others are down.
    #[allow(clippy::too_many_arguments)]
    fn step_feet(
        &mut self,
        now: f64,
        dt: f32,
        turned: f32,
        v: Vec2,
        spin: f32,
        p: &Pattern,
    ) -> [Plant; 4] {
        // Planted feet (and the spot a landing foot is headed for) stay put on the ground as
        // the body goes on, and so does a foot only just peeling off it; one clear of the
        // ground swings from where it left it, under the body.
        for f in &mut self.feet {
            let clear = f.air.is_some_and(|(t0, dur, h)| {
                h * swing_height(((now - t0) as f32 / dur).clamp(0.0, 1.0)) > 0.012
            });
            if !clear {
                f.at = turn(f.at, -turned) - v * dt;
            }
            if let Some(a) = &mut f.aim {
                *a = turn(*a, -turned) - v * dt;
            }
        }
        let phase = |cycles: f64, k: usize| {
            let c = cycles - f64::from(p.offsets[k]);
            (c.rem_euclid(1.0) as f32, c.floor() as i64)
        };
        if p.stepping {
            if !self.stepping {
                // Setting off: the foot furthest behind steps first and the rhythm picks up
                // from it; feet the rhythm has in the air wait for their next turn.
                let ahead = if v.length() > 1e-3 {
                    v.normalize()
                } else {
                    Vec2::NEG_Y
                };
                let behind = |k: usize| {
                    -(self.feet[k].at - p.home[k]).dot(ahead) + if k >= 2 { 0.02 } else { 0.0 }
                };
                let first = (0..4)
                    .max_by(|&a, &b| behind(a).total_cmp(&behind(b)))
                    .unwrap_or(2);
                let (t, _) = phase(self.cycles, first);
                self.cycles += f64::from((p.duty[first] - t).rem_euclid(1.0));
                for k in 0..4 {
                    let (t, lap) = phase(self.cycles, k);
                    self.feet[k].lap = if k != first && t > p.duty[k] {
                        lap
                    } else {
                        lap - 1
                    };
                }
            }
            self.cycles += f64::from(p.freq * dt);
        }
        self.stepping = p.stepping;
        // Land where the swing was headed (a frame's fraction ago).
        for k in 0..4 {
            if let Some((t0, dur, _)) = self.feet[k].air
                && now >= t0 + f64::from(dur)
            {
                let left = (t0 + f64::from(dur) - now) as f32;
                let f = &mut self.feet[k];
                f.at = f
                    .aim
                    .take()
                    .unwrap_or_else(|| Self::landing(p, k, v, spin, left));
                f.air = None;
                f.down = now;
            }
        }
        // Lift, the foot furthest out of place first.
        let off = |f: &Foot, k: usize| (f.at - p.home[k]).length();
        let mut order = [0, 1, 2, 3];
        order.sort_by(|&a, &b| off(&self.feet[b], b).total_cmp(&off(&self.feet[a], a)));
        let mut airborne = self.feet.iter().filter(|f| f.air.is_some()).count();
        for k in order {
            let f = self.feet[k];
            if f.air.is_some() {
                continue;
            }
            let out = off(&f, k);
            let (t, lap) = phase(self.cycles, k);
            let freq = p.freq.max(1e-3);
            let lift = if out > p.reach[k] {
                // As far as the leg goes: step now.
                Some(((1.0 - p.duty[k]) / freq).clamp(0.08, 0.4))
            } else if p.stepping {
                (t >= p.duty[k] && lap != f.lap && now - f.down > 0.03)
                    .then(|| ((1.0 - t) / freq).clamp(0.06, 0.6))
            } else {
                (airborne == 0 && out > p.tidy && now - f.down > 0.15).then_some(p.tidy_secs)
            };
            if let Some(dur) = lift {
                let h = if p.stepping {
                    p.lift[k]
                } else {
                    p.lift[k] * 0.6
                };
                self.feet[k].air = Some((now, dur, h));
                self.feet[k].aim = None;
                self.feet[k].lap = if p.stepping { lap } else { f.lap };
                airborne += 1;
            }
        }
        // No foot down: the body flies until the first one lands.
        if self.feet.iter().any(|f| f.air.is_none()) {
            self.flight = None;
        } else if self.flight.is_none() {
            let land = self
                .feet
                .iter()
                .filter_map(|f| f.air.map(|(t0, d, _)| t0 + f64::from(d)))
                .fold(f64::MAX, f64::min);
            self.flight = Some((now, (land - now).max(0.0) as f32));
        }
        // Late in the swing the spot it comes down on is settled.
        for (k, f) in self.feet.iter_mut().enumerate() {
            if let Some((t0, dur, _)) = f.air
                && f.aim.is_none()
                && (now - t0) as f32 >= 0.8 * dur
            {
                let left = (t0 + f64::from(dur) - now) as f32;
                f.aim = Some(Self::landing(p, k, v, spin, left));
            }
        }
        std::array::from_fn(|k| {
            let f = self.feet[k];
            match f.air {
                Some((t0, dur, h)) => {
                    let u = ((now - t0) as f32 / dur).clamp(0.0, 1.0);
                    let left = (t0 + f64::from(dur) - now) as f32;
                    let to = f.aim.unwrap_or_else(|| Self::landing(p, k, v, spin, left));
                    // Up off the spot, across, and straight down onto the new one.
                    let along = smooth(0.06, 0.86, u);
                    Plant {
                        at: f.at.lerp(to, along),
                        lift: h * swing_height(u),
                        u,
                        down: false,
                    }
                }
                None => {
                    let u = if self.stepping {
                        (phase(self.cycles, k).0 / p.duty[k].max(1e-3)).min(1.0)
                    } else {
                        0.5
                    };
                    Plant {
                        at: f.at,
                        lift: 0.0,
                        u,
                        down: true,
                    }
                }
            }
        })
    }

    /// Height (rig m) of the arc the body flies while no foot is down.
    fn flight_arc(&self, now: f64, scale: f32) -> f32 {
        self.flight.map_or(0.0, |(t0, dur)| {
            let u = ((now - t0) as f32 / dur.max(1e-3)).clamp(0.0, 1.0);
            9.81 * dur * dur * 0.5 * u * (1.0 - u) / scale
        })
    }
}

/// How high a swinging foot is (share of its lift) `u` of the way through its swing: up off
/// the spot quickly, highest a little before halfway, and straight down onto the new one.
fn swing_height(u: f32) -> f32 {
    (PI * u).sin().max(0.0).powf(0.8) * (1.15 - 0.3 * u)
}

/// How far the body comes down (rig m) for a foot to reach its spot, `need` taking it all the
/// way: all of it for a planted foot, and some for one just lifting off or about to land, so
/// it leaves and meets the ground cleanly without the body ducking after a spot far ahead.
fn reach_need(pl: &Plant, need: f32, leg: f32) -> f32 {
    if pl.down {
        need
    } else {
        let w = (1.0 - smooth(0.05, 0.35, pl.u)).max(smooth(0.55, 0.95, pl.u));
        need.min(0.12 * leg) * w
    }
}

/// Phase `b` written as near to `a` as it goes (whole cycles apart are the same moment).
fn near_phase(a: f32, b: f32) -> f32 {
    b + (a - b).round()
}

fn mix_phases(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    std::array::from_fn(|k| lerp(a[k], near_phase(a[k], b[k]), t))
}

// ======================================================================= legs

/// A leg solved: angles of its segments in the leg's plane, the joint it hangs from (rest
/// coordinates; the shoulder rides on the shoulder blade) and how far the leg's plane is
/// turned out sideways about it (rad).
#[derive(Debug, Clone, Copy)]
struct LegSolve<const N: usize> {
    ang: [f32; N],
    root: Vec3,
    abduct: f32,
}

/// Rest angles and lengths of a chain's segments in the YZ plane.
fn chain<const N: usize>(j: &[Vec3]) -> ([f32; N], [f32; N]) {
    let mut rest = [0.0; N];
    let mut len = [0.0; N];
    for i in 0..N {
        let d = yz(j[i + 1]) - yz(j[i]);
        rest[i] = angle(d);
        len[i] = d.length();
    }
    (rest, len)
}

/// How far the leg's plane turns about `root` (along Z) so a chain whose toe sits `c` out
/// from the root (X) reaches `toe`, and the toe in the turned plane.
fn abduct(root: Vec3, toe: Vec3, c: f32) -> (f32, Vec2) {
    let d = toe - root;
    let r = Vec2::new(d.x, d.y).length();
    let mut a = 0.0;
    if r > 1e-4 {
        let phi = d.y.atan2(d.x);
        let b = (c / r).clamp(-1.0, 1.0).acos();
        let (a1, a2) = (wrap_pi(phi + b), wrap_pi(phi - b));
        a = if a1.abs() < a2.abs() { a1 } else { a2 };
        a = a.clamp(-0.6, 0.6);
    }
    let y = -d.x * a.sin() + d.y * a.cos();
    (a, Vec2::new(root.y + y, toe.z))
}

/// Two bones from `root` to `end` (YZ), bending to `bend`'s side: the angles of both.
fn two_bone(root: Vec2, end: Vec2, l0: f32, l1: f32, bend: f32) -> (f32, f32) {
    let d = end - root;
    let dist = d.length().clamp((l0 - l1).abs() + 1e-4, l0 + l1 - 1e-4);
    let cos_a = ((l0 * l0 + dist * dist - l1 * l1) / (2.0 * l0 * dist)).clamp(-1.0, 1.0);
    let a0 = angle(d) + bend * cos_a.acos();
    let j1 = root + dir(a0) * l0;
    (a0, angle(end - j1))
}

/// A deer's or wolf's leg from its shoulder or hip (`root`) to the toe at `toe` (rest
/// coordinates), with the pastern turned `foot_d` from rest and the carpus or hock folded
/// `flex`. Where the toe is out of reach of the folded leg, the cannon and then the pastern
/// straighten toward it.
fn solve_quad_leg(
    j: &[Vec3; 5],
    root: Vec3,
    toe: Vec3,
    foot_d: f32,
    flex: f32,
    front: bool,
) -> LegSolve<4> {
    let (rest, len) = chain::<4>(j);
    let (abd, t) = abduct(root, toe, j[4].x - root.x);
    let r = yz(root);
    let reach = len[0] + len[1] - 2e-3;
    let far = |j2: Vec2| (j2 - r).length() > reach;
    let mut a3 = rest[3] + foot_d;
    let mut f3 = t - dir(a3) * len[3];
    let ds = wrap_pi(angle(f3 - r) - angle(yz(j[3]) - yz(j[0])));
    let mut a2 = rest[2] + ds + flex;
    let j2_of = |a2: f32, f3: Vec2| f3 - dir(a2) * len[2];
    if far(j2_of(a2, f3)) {
        // Turn the cannon toward the root as far as it takes.
        let straight = angle(f3 - r);
        let at = |w: f32| a2 + wrap_pi(straight - a2) * w;
        if !far(j2_of(straight, f3)) {
            let (mut lo, mut hi) = (0.0, 1.0);
            for _ in 0..14 {
                let mid = 0.5 * (lo + hi);
                if far(j2_of(at(mid), f3)) {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            a2 = at(hi);
        } else {
            // And the pastern too.
            let line = angle(t - r);
            let f3_at = |w: f32| t - dir(a3 + wrap_pi(line - a3) * w) * len[3];
            let (mut lo, mut hi) = (0.0, 1.0);
            for _ in 0..14 {
                let mid = 0.5 * (lo + hi);
                let f = f3_at(mid);
                if far(j2_of(angle(f - r), f)) {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            a3 += wrap_pi(line - a3) * hi;
            f3 = t - dir(a3) * len[3];
            a2 = angle(f3 - r);
        }
    }
    let j2 = j2_of(a2, f3);
    let (a0, a1) = two_bone(r, j2, len[0], len[1], if front { -1.0 } else { 1.0 });
    LegSolve {
        ang: [a0, a1, a2, a3],
        root,
        abduct: abd,
    }
}

// ======================================================================= deer, wolves, boars

/// Which four-legged body a pose is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Beast {
    Deer,
    Wolf,
    Boar,
}

impl Beast {
    /// A value by beast.
    fn by(self, deer: f32, wolf: f32, boar: f32) -> f32 {
        match self {
            Beast::Deer => deer,
            Beast::Wolf => wolf,
            Beast::Boar => boar,
        }
    }
}

/// How a four-legged species moves (rig meters and seconds).
struct Build {
    /// Footfalls of its gaits from slowest to fastest: when each foot (LF, RF, LH, RH)
    /// touches down in the cycle.
    gaits: &'static [[f32; 4]],
    /// Speeds (rig m/s) where it changes into each next gait, and over how wide a band.
    shifts: &'static [(f32, f32)],
    /// Index of the gallop in `gaits`.
    gallop: f32,
    /// Stance length: the longest (rig m), and the speed at which it is half that.
    stride: (f32, f32),
    /// Duty factor at speed v: `a + b / (1 + (v / c)^1.5)`.
    duty: (f32, f32, f32),
    /// Swing lift (front, hind) walking and galloping (rig m).
    lift: [f32; 2],
    lift_fast: [f32; 2],
    /// How far a planted foot may be from its place before it must step (front, hind).
    reach: [f32; 2],
    /// The shoulder blade carries the shoulder this far along the body (rig m).
    blade: f32,
    /// Body lowered at a gallop (rig m), and its bounce at a trot and a gallop.
    low: f32,
    bounce: (f32, f32),
    /// How far the spine flexes and extends in the gallop (rad at the croup), and when in the
    /// gallop's cycle it is most flexed (the gathered flight, just before the hind feet land).
    flex: f32,
    gather: f32,
    /// At a gallop the hind feet come down this much further forward and further out (rig
    /// m), reaching under the chest past the fore feet.
    tuck: (f32, f32),
    /// At a gallop the fore feet come down this much further back (rig m), under the chest
    /// rather than reaching out ahead of it.
    fore_back: f32,
    /// How the neck follows the spine through the gallop: raised (+) or lowered as it flexes
    /// (rad per rad), the head turned back level.
    nod: f32,
}

/// Lateral-sequence walk: left hind, left fore, right hind, right fore.
const WALK: [f32; 4] = [0.25, 0.75, 0.0, 0.5];
/// Diagonal pairs together.
const TROT: [f32; 4] = [0.5, 1.0, 0.0, 0.5];
/// Canter with the left fore leading: right hind, then left hind with right fore, then left
/// fore.
const LOPE: [f32; 4] = [0.2, 1.0, 0.0, 0.8];
/// Rotary gallops: the footfalls go round the body (hind lead on one side, fore lead on the
/// other).
const ROTARY_DEER: [f32; 4] = [0.55, 1.45, 0.0, 0.1];
const ROTARY_WOLF: [f32; 4] = [0.35, 1.45, 0.0, 0.9];
/// Transverse gallop: the footfalls cross the body the same way front and back (a pig's).
const TRANSVERSE: [f32; 4] = [0.45, 0.55, 0.0, 0.1];
/// A bounding leap: the hind feet push off together, the fore feet land together.
const BOUND: [f32; 4] = [0.42, 0.45, 0.0, 0.02];

const DEER_BUILD: Build = Build {
    gaits: &[WALK, TROT, ROTARY_DEER],
    shifts: &[(1.9, 0.35), (5.0, 0.7)],
    gallop: 2.0,
    stride: (0.95, 1.2),
    duty: (0.2, 0.55, 2.2),
    lift: [0.08, 0.065],
    lift_fast: [0.17, 0.14],
    reach: [0.47, 0.52],
    blade: 0.065,
    low: 0.085,
    bounce: (0.025, 0.05),
    flex: 0.18,
    gather: 0.9,
    tuck: (0.05, 0.015),
    fore_back: 0.0,
    nod: 0.5,
};

const WOLF_BUILD: Build = Build {
    gaits: &[WALK, TROT, LOPE, ROTARY_WOLF],
    shifts: &[(2.0, 0.35), (4.6, 0.5), (7.8, 0.7)],
    gallop: 3.0,
    stride: (0.78, 1.1),
    duty: (0.2, 0.5, 2.4),
    lift: [0.06, 0.05],
    lift_fast: [0.12, 0.11],
    reach: [0.37, 0.42],
    blade: 0.045,
    low: 0.06,
    bounce: (0.02, 0.04),
    flex: 0.3,
    gather: 0.85,
    tuck: (0.13, 0.03),
    fore_back: 0.0,
    nod: -0.35,
};

/// Short-legged and stiff-backed: quick, short strides.
const BOAR_BUILD: Build = Build {
    gaits: &[WALK, TROT, TRANSVERSE],
    shifts: &[(1.6, 0.3), (4.4, 0.6)],
    gallop: 2.0,
    stride: (0.4, 0.9),
    duty: (0.2, 0.55, 2.0),
    lift: [0.05, 0.045],
    lift_fast: [0.15, 0.1],
    reach: [0.24, 0.27],
    blade: 0.04,
    low: 0.02,
    bounce: (0.02, 0.024),
    flex: 0.14,
    gather: 0.9,
    tuck: (0.06, 0.015),
    fore_back: 0.07,
    nod: 0.2,
};

/// Piglets scamper: the same gaits on short, quick strides, bouncing high off each stance
/// and held up rather than stretched out low.
const PIGLET_BUILD: Build = Build {
    stride: (0.3, 0.8),
    lift: [0.06, 0.055],
    lift_fast: [0.14, 0.13],
    reach: [0.2, 0.22],
    low: 0.0,
    bounce: (0.03, 0.065),
    flex: 0.09,
    tuck: (0.03, 0.01),
    // A piglet's short legs reach no further back under it than they already do.
    fore_back: 0.0,
    ..BOAR_BUILD
};

impl Build {
    /// The gait index (0 walk, 1 trot, ...) it settles into at `speed` (rig m/s).
    fn gait_at(&self, speed: f32) -> f32 {
        self.shifts
            .iter()
            .map(|&(c, w)| smooth(c - w, c + w, speed))
            .sum()
    }

    /// What the feet do at gait `g` and speed `v` (rig m/s); `bound` 0..1 turns the gait
    /// into leaps.
    fn pattern(&self, rig: &QuadRig, g: f32, v: f32, bound: f32) -> Pattern {
        let last = self.gaits.len() - 1;
        let i0 = (g.max(0.0).floor() as usize).min(last);
        let i1 = (i0 + 1).min(last);
        let mut offsets = mix_phases(self.gaits[i0], self.gaits[i1], g - i0 as f32);
        if bound > 0.0 {
            offsets = mix_phases(offsets, BOUND, bound);
        }
        let duty = self.duty.0 + self.duty.1 / (1.0 + (v / self.duty.2).powf(1.5));
        let duty = lerp(duty, 0.3, bound);
        let stride = self.stride.0 * v / (v + self.stride.1);
        let stepping = v > 0.05;
        let freq = if stepping {
            v * duty / stride.max(1e-3)
        } else {
            0.0
        };
        let fast = smooth(self.gallop - 1.0, self.gallop, g).max(bound);
        let lift = |k: usize| {
            let e = usize::from(k >= 2);
            lerp(self.lift[e], self.lift_fast[e], fast) * (1.0 + 0.5 * bound)
        };
        let home = std::array::from_fn(|k| {
            let toe = rig.leg(k as u8)[4];
            let (fwd, out) = if k >= 2 {
                (self.tuck.0 * fast, self.tuck.1 * fast)
            } else {
                (-self.fore_back * fast, 0.0)
            };
            Vec2::new(toe.x + out * toe.x.signum(), toe.z - fwd)
        });
        Pattern {
            offsets,
            duty: [duty; 4],
            freq,
            stepping,
            lift: std::array::from_fn(lift),
            home,
            reach: [self.reach[0], self.reach[0], self.reach[1], self.reach[1]],
            tidy: 0.05,
            tidy_secs: 0.3,
        }
    }
}

/// Where the head, ears and tail are, relative to the rest pose (radians).
#[derive(Debug, Clone, Copy, Default)]
struct Look {
    neck: f32,
    neck2: f32,
    head: f32,
    /// Head turned left (+) or right, the neck turning with it.
    yaw: f32,
    /// The head turned on further by itself (rad, left positive).
    gaze: f32,
    /// Ears turned forward (+) or back (−), each, and raised (+) or drooping (−).
    ears: [f32; 2],
    ear_up: f32,
    /// Tail raised (+).
    tail: f32,
    tail2: f32,
    tail_swish: f32,
    jaw: f32,
    /// Forequarters lowered (body pitched nose-down), radians.
    stoop: f32,
    /// A boar's crests raised: 0 lying in the coat, 1 standing up.
    bristle: f32,
}

impl Look {
    fn mix(self, o: Look, t: f32) -> Look {
        let l = |a: f32, b: f32| a + (b - a) * t;
        Look {
            neck: l(self.neck, o.neck),
            neck2: l(self.neck2, o.neck2),
            head: l(self.head, o.head),
            yaw: l(self.yaw, o.yaw),
            gaze: l(self.gaze, o.gaze),
            ears: [l(self.ears[0], o.ears[0]), l(self.ears[1], o.ears[1])],
            ear_up: l(self.ear_up, o.ear_up),
            tail: l(self.tail, o.tail),
            tail2: l(self.tail2, o.tail2),
            tail_swish: l(self.tail_swish, o.tail_swish),
            jaw: l(self.jaw, o.jaw),
            stoop: l(self.stoop, o.stoop),
            bristle: l(self.bristle, o.bristle),
        }
    }

    fn ear_fwd(self, a: f32) -> Look {
        Look {
            ears: [a, a],
            ..self
        }
    }
}

/// What the head and ears attend to: the bearing (rad from ahead, right positive) and how
/// much.
fn attention(k: &Tick, m: &Motion, i: &Input) -> Option<(f32, f32)> {
    let noise = m.noise.and_then(|(at, b)| {
        let ago = (k.now - at) as f32;
        (ago < 3.5).then_some((b, 1.0 - smooth(2.5, 3.5, ago)))
    });
    let alarmed = matches!(
        i.state,
        AnimalState::Alert | AnimalState::Flee | AnimalState::Rest
    ) || i.alarmed < 4.0;
    let threat = k
        .threat
        .filter(|_| alarmed && i.danger > 0.3)
        .map(|b| (b, 1.0 - smooth(4.0, 6.0, i.alarmed)));
    match (noise, threat) {
        (Some(n), Some(t)) => Some(if n.1 > t.1 { n } else { t }),
        (n, t) => n.or(t),
    }
}

/// A posed deer, wolf or boar.
struct Quad<'a> {
    rig: &'a QuadRig,
    b: &'a Build,
    beast: Beast,
    seed: f32,
    body: Frame,
    look: Look,
    plants: [Plant; 4],
    /// Each leg's pastern turn and carpus or hock fold.
    style: [(f32, f32); 4],
    /// The spine flexed (+, gathered) or extended (−), radians at the croup.
    flex: f32,
    lie: f32,
    dead: f32,
    /// Killed this long ago (s), falling onto this side (+1 its right side up).
    fall: Option<(f32, f32)>,
    upright: f32,
}

#[allow(clippy::too_many_arguments)]
fn quad(
    rig: &QuadRig,
    b: &Build,
    beast: Beast,
    i: &Input,
    m: &mut Motion,
    k: &Tick,
    out: &mut Pose,
) {
    let st = i.state;
    let scale = body_scale(i.species, i.male, i.young);
    // Killed: how long ago, and how limp it has gone.
    let fall = (st == AnimalState::Dead).then_some(k.t);
    let dead = fall.map_or(0.0, |t| smooth(0.1, 0.7, t));
    let lie = if st == AnimalState::Rest {
        smooth(0.0, 1.8, k.t)
    } else {
        0.0
    };
    let upright = 1.0 - lie.max(dead);
    let crouching = if st == AnimalState::Stalk || i.crouch {
        1.0
    } else {
        0.0
    };
    // A wolf rushing from its stalk stays low through its first bounds.
    let rush = m.startle && beast == Beast::Wolf && st == AnimalState::Chase;
    if k.fresh {
        m.crouch = crouching;
    } else {
        ease_to(
            &mut m.crouch,
            crouching,
            if rush { 0.45 } else { 0.25 },
            k.dt,
        );
    }
    let crouch = m.crouch;
    // Gait: by speed (turning on the spot counts), eased from one to the next.
    let v_eff =
        (k.speed * k.speed + (m.spin * 0.4).powi(2)).sqrt() * smooth(0.6, 0.0, lie.max(dead));
    let want = b.gait_at(v_eff);
    if k.fresh {
        m.gait = want;
    } else {
        ease_to(&mut m.gait, want, 0.22, k.dt);
    }
    if m.state != Some(st) {
        // Bolting from a stand, or rushing from a stalk: the first bounds are leaps.
        let from_stalk = m.state == Some(AnimalState::Stalk);
        m.startle = m.vel.length() < 2.5
            && match beast {
                Beast::Deer => st == AnimalState::Flee,
                Beast::Wolf => st == AnimalState::Chase && from_stalk,
                Beast::Boar => false,
            };
        m.state = Some(st);
        // The head and tail go from where they were to the new state's.
        if !k.fresh {
            m.fade = Some((k.now, m.was));
        }
    }
    let bound = if m.startle && matches!(st, AnimalState::Flee | AnimalState::Chase) {
        1.0 - smooth(0.45, 0.85, k.t)
    } else {
        0.0
    };
    let mut p = b.pattern(rig, m.gait, v_eff, bound);
    if fall.is_some() {
        // The dead don't step.
        p.reach = [f32::MAX; 4];
        p.tidy = f32::MAX;
    }
    // Hurt: shoved away from the blow, it staggers a step or two that way, and the stride
    // under way hitches (the rhythm checks for a beat before it picks up again).
    let size = rig.half_width / 0.2;
    if let Some((t, from)) = k.flinch {
        let shove = 0.14 * size * smooth(0.0, 0.08, t) * (1.0 - smooth(0.5, 0.9, t));
        let away = Vec2::new(-from.sin(), from.cos());
        for h in &mut p.home {
            *h += away * shove;
        }
        let hitch = smooth(0.12, 0.25, t) * (1.0 - smooth(0.4, 0.65, t));
        p.freq *= 1.0 - 0.6 * hitch;
    }
    if k.fresh {
        m.cycles = f64::from(i.phase / TAU);
        m.reset_feet(k.now, k.v, &p);
    }
    // Killed on the move, the feet buckle under the body and skid on with it.
    let plants = if fall.is_some() {
        m.step_feet(k.now, k.dt, 0.0, Vec2::ZERO, 0.0, &p)
    } else {
        m.step_feet(k.now, k.dt, k.turned, k.v, m.spin, &p)
    };
    let stepping = smooth(0.03, 0.3, v_eff) * upright;
    let run = smooth(0.4, 1.0, m.gait);
    let fast = smooth(b.gallop - 0.8, b.gallop, m.gait).max(bound);

    let look = quad_look(beast, i, k, m, crouch, fast, run, plants, stepping);

    // The body rides on the feet: it sinks into each stance in the running gaits (front and
    // hind apart, so it pitches over the pair carrying it), vaults gently over the walk and
    // flies an arc when no foot is down.
    let mut comp = [0.0f32; 2];
    for (n, pl) in plants.iter().enumerate() {
        if pl.down && p.stepping {
            comp[n / 2] = comp[n / 2].max((PI * pl.u).sin());
        }
    }
    let bounce = lerp(b.bounce.0, b.bounce.1, fast) * run * stepping * (1.0 + bound);
    let vault = 0.012 * (1.0 - run) * stepping;
    let dy_f = -bounce * comp[0] + vault * (comp[0] - 0.6);
    let dy_h = -bounce * comp[1] + vault * (comp[1] - 0.6);
    let arc = m.flight_arc(k.now, scale) * upright;
    let low = (b.low * fast + 0.03 * run * (1.0 - fast)) * stepping;
    // Weight thrown back setting off and forward stopping, settling after.
    let a_fwd = -m.accel.y;
    m.lean
        .step((0.01 * a_fwd).clamp(-0.11, 0.11) * upright, 1.3, 0.38, k.dt);
    m.surge.step(
        (0.004 * a_fwd).clamp(-0.05, 0.05) * upright,
        1.3,
        0.42,
        k.dt,
    );
    // Leaning into turns.
    let mut roll = (0.028 * -m.vel.y * m.spin).clamp(-0.16, 0.16) * upright;
    // Breathing, quicker and deeper after a hard run.
    let exert = ((1.0 - i.stamina) * 1.6).clamp(0.0, 1.0);
    if k.fresh {
        m.breath = k.seed;
    }
    m.breath = (m.breath + lerp(0.33, 1.7, exert) * k.dt).fract();
    let breath = (0.0025 + 0.0045 * exert) * (TAU * m.breath).sin();
    // The spine flexes through the gathered flight of the gallop (fore feet swept back under
    // the chest, hind feet swinging up past them to land) and extends through the extended
    // one (hind feet pushed off behind, fore feet reaching out).
    let cycle = m.cycles.rem_euclid(1.0) as f32;
    let mut flex = b.flex * fast * stepping * (TAU * (cycle - b.gather)).cos();

    let lz = rig.hind.joints[0].z - rig.front.joints[0].z;
    let mut pitch = (dy_f - dy_h) / lz + m.lean.x - look.stoop;
    let mut pos = rig.body + Vec3::new(0.0, (dy_f + dy_h) * 0.5 + arc - low + breath, m.surge.x);
    let sink = beast.by(0.12, 0.13, 0.1) * crouch;
    pos.y -= sink;
    pitch -= 0.04 * crouch;
    if rush {
        // Coiled for an instant, then away.
        pos.y -= 0.04 * (1.0 - smooth(0.05, 0.25, k.t));
    }
    if let Some((t, from)) = k.flinch {
        // Hurt: jolted away from the blow, rolling off it, hunched (the croup tucked under)
        // and ducking as the legs give a little, then gathering itself.
        use Ease::{Both, Out};
        let jolt = track(
            t,
            &[
                (0.0, 0.0, Both),
                (0.05, 1.0, Out),
                (0.14, 0.75, Both),
                (0.6, 0.0, Both),
            ],
        );
        let duck = track(t, &[(0.0, 0.0, Both), (0.07, 1.0, Out), (0.35, 0.0, Both)]);
        // Then it stumbles: a foreleg gives under the shove, the forequarters dipping, and it
        // catches itself.
        let stumble = track(
            t,
            &[
                (0.0, 0.0, Both),
                (0.12, 0.0, Both),
                (0.24, 1.0, Out),
                (0.55, 0.0, Both),
            ],
        );
        pos += Vec3::new(-from.sin(), 0.0, from.cos()) * 0.13 * size * jolt;
        pos.y -= 0.08 * size * duck + 0.08 * size * stumble;
        let sway = track(t, &[(0.0, 0.0, Both), (0.1, 1.0, Out), (0.6, 0.0, Both)]);
        roll += from.sin() * (0.24 * jolt + 0.08 * sway);
        // From behind, the rump jumps up.
        pitch -= 0.06 * jolt * (-from.cos()).max(0.0) + 0.22 * stumble;
        flex += 0.22 * duck;
    }
    let mut rot = Quat::from_rotation_z(roll) * Quat::from_rotation_x(pitch);
    if lie > 0.0 {
        pos.y -= (rig.belly - beast.by(0.1, 0.06, 0.04)) * lie;
        rot = Quat::from_rotation_x(pitch * (1.0 - lie) + 0.03 * lie);
    }
    if let Some(t) = fall {
        // Killed: the legs give, the forequarters first (on the run it pitches onto its
        // chest and knees, carried on by its speed), then the hindquarters; it topples onto
        // its side, lands, rocks back off the ground and settles.
        use Ease::{Both, In, Out};
        let side = k.side;
        let running = smooth(2.0, 8.0, m.struck);
        // How far the front and back of the body come down: onto the knees, then lying.
        let drop = rig.half_width + 0.02 - pos.y;
        let knees = -0.5 * rig.belly * (1.0 + 0.3 * running);
        let haunch = -0.45 * rig.belly;
        let front = track(
            t,
            &[
                (0.0, 0.0, Both),
                (lerp(0.3, 0.22, running), knees, In),
                (0.4, knees, Both),
                (0.62, drop, In),
            ],
        );
        let back = track(
            t,
            &[
                (0.0, 0.0, Both),
                (lerp(0.04, 0.14, running), 0.0, Both),
                (lerp(0.33, 0.42, running), haunch, In),
                (0.62, drop, In),
            ],
        );
        let up = 0.05 * rig.belly;
        let bounce = track(
            t,
            &[
                (0.62, 0.0, Both),
                (0.7, up, Out),
                (0.8, 0.0, In),
                (0.88, 0.25 * up, Out),
                (0.96, 0.0, In),
            ],
        );
        pos.y += (front + back) * 0.5 + bounce;
        let roll = side
            * track(
                t,
                &[
                    (0.0, 0.0, Both),
                    (0.3, 0.1, Both),
                    (0.62, FRAC_PI_2, In),
                    (0.71, FRAC_PI_2 - 0.13, Out),
                    (0.82, FRAC_PI_2 + 0.03, Both),
                    (0.94, FRAC_PI_2, Both),
                ],
            );
        let nose = pitch * (1.0 - smooth(0.0, 0.3, t)) + (front - back) / lz;
        rot = Quat::from_rotation_z(roll) * Quat::from_rotation_x(nose);
    }
    let mut body = Frame {
        origin: rig.body,
        pos,
        rot,
    };
    // Each leg's fold: swinging legs fold at the carpus or hock and turn the pastern under;
    // a stance foot rolls over its toe as it pushes off.
    let g_flex = lerp(0.9, 1.6, run);
    let style: [(f32, f32); 4] = std::array::from_fn(|n| {
        let front = n < 2;
        let pl = plants[n];
        if pl.down {
            (-0.45 * smooth(0.65, 1.0, pl.u) * stepping, 0.0)
        } else {
            // At speed a hind leg pushes off extended and peels off the ground before it
            // folds (folding at once would drag the hock along the ground behind).
            let late = if front { 0.0 } else { 0.18 * fast };
            let u = ((pl.u - late) / (1.0 - late)).max(0.0);
            let fold = (PI * u.powf(0.75)).sin().max(0.0) * if p.stepping { 1.0 } else { 0.5 };
            (
                -(if front { 1.3 } else { 0.9 }) * fold,
                g_flex * fold * if front { -1.0 } else { 0.75 },
            )
        }
    });
    let mut q = Quad {
        rig,
        b,
        beast,
        seed: k.seed,
        body,
        look,
        plants,
        style,
        flex,
        lie,
        dead,
        fall: fall.map(|t| (t, k.side)),
        upright,
    };
    // The body comes down as far as a planted foot needs to reach the ground.
    if upright > 0.0 {
        for _ in 0..2 {
            let (df, dh) = q.reach_drop();
            if df <= 0.0 && dh <= 0.0 {
                break;
            }
            body.pos.y -= (df + dh) * 0.5;
            body.rot *= Quat::from_rotation_x(-(df - dh) / lz);
            q.body = body;
        }
    }
    // Head and neck steady against the bob and pitch of the body (crouching, it keeps them
    // down with it).
    let bob = body.pos.y - rig.body.y + low + sink;
    let tilt = (body.rot * Vec3::NEG_Z).y.asin() + look.stoop;
    q.look.neck -= (1.4 * bob + 0.6 * tilt) * upright;
    q.look.head -= 0.3 * tilt * upright;
    q.place(out);
}

/// How a deer's, wolf's or boar's head, ears and tail look this step.
#[allow(clippy::too_many_arguments)]
fn quad_look(
    beast: Beast,
    i: &Input,
    k: &Tick,
    m: &mut Motion,
    crouch: f32,
    gallop: f32,
    run: f32,
    plants: [Plant; 4],
    stepping: f32,
) -> Look {
    let st = i.state;
    let deer = beast == Beast::Deer;
    let wolf = beast == Beast::Wolf;
    let boar = beast == Beast::Boar;
    let moving = smooth(0.25, 0.8, k.speed);
    let ease = |secs: f32| smooth(0.0, secs, k.t);
    let alarmed = i.alarmed < 3.0 && i.danger >= 0.5;
    // Deer swish their tails at flies in bouts, and a boar's tail twitches on and on; wolves
    // hold theirs still.
    let bout = {
        let x = (k.clock * 0.19 + k.vary * 5.0).fract() * 5.3;
        smooth(0.0, 0.15, x) * (1.0 - smooth(1.1, 1.4, x))
    };
    let swish = match beast {
        Beast::Deer => 0.35 * bout * (k.clock * TAU * 2.3).sin(),
        Beast::Wolf => 0.0,
        Beast::Boar => 0.4 * (0.4 + 0.6 * bout) * (k.clock * TAU * 1.7).sin(),
    };
    let calm = Look {
        yaw: 0.25 * drift(k.clock, 0.23, k.seed),
        tail_swish: swish,
        ..Look::default()
    }
    .ear_fwd(0.15);
    let mut l = calm;
    // Walking: head a little lower, nodding twice a stride as each fore foot takes the
    // weight.
    let fore = plants[0].down as u8 as f32 * (PI * plants[0].u).sin()
        + plants[1].down as u8 as f32 * (PI * plants[1].u).sin();
    let nod = (0.05 * (1.0 - run) + 0.03 * gallop) * (fore - 0.6) * stepping;
    let walk = Look {
        neck: beast.by(-0.12, -0.15, -0.06) - nod,
        head: 0.08 + 0.6 * nod,
        yaw: 0.0,
        ..calm
    }
    .ear_fwd(0.1);
    l = l.mix(walk, moving);
    // Grazing deer look up now and then to chew and watch.
    let watch = if deer && matches!(st, AnimalState::Graze | AnimalState::Drink) {
        let x = k.t - 7.0 - 3.0 * k.vary;
        if x > 0.0 {
            let w = x % (10.0 + 4.0 * k.vary);
            smooth(0.0, 0.4, w) * (1.0 - smooth(2.6, 3.2, w))
        } else {
            0.0
        }
    } else {
        0.0
    };
    let chew = (k.clock * beast.by(9.0, 7.0, 6.0)).sin();
    let target = match st {
        AnimalState::Graze | AnimalState::Drink if boar => {
            // Rooting: the snout shoved into the soil and levered up out of it, over and over,
            // sweeping from side to side, the jaw working on what it turns up; every so often
            // the head comes up to scent the air.
            let c = (k.clock / 1.3 + k.seed * 3.1).fract();
            let shove = smooth(0.0, 0.55, c) * (1.0 - smooth(0.6, 0.75, c));
            let flick = smooth(0.6, 0.72, c) * (1.0 - smooth(0.75, 1.0, c));
            let sniff = {
                let x = k.t - 4.0 - 3.0 * k.vary;
                if x > 0.0 {
                    let w = x % (7.0 + 3.0 * k.vary);
                    smooth(0.0, 0.35, w) * (1.0 - smooth(1.4, 1.9, w))
                } else {
                    0.0
                }
            };
            let down = Look {
                neck: ROOT.0 - 0.14 * shove + 0.04 * flick,
                head: ROOT.1 + 0.04 * shove + 0.2 * flick,
                stoop: ROOT.2,
                yaw: 0.25 * drift(k.clock, 0.9, k.seed),
                ear_up: -0.1,
                jaw: -0.05 - 0.05 * chew,
                tail_swish: calm.tail_swish,
                ..Look::default()
            }
            .ear_fwd(0.25);
            let up = Look {
                neck: -0.1,
                // The nose working at the air.
                head: 0.25 + 0.05 * (k.clock * 11.0).sin().max(0.0),
                yaw: 0.3 * drift(k.clock, 0.35, k.seed),
                ear_up: 0.15,
                tail_swish: calm.tail_swish,
                ..Look::default()
            }
            .ear_fwd(0.5);
            Some((down.mix(up, sniff).mix(l, smooth(0.5, 1.0, k.speed)), 0.8))
        }
        AnimalState::Graze | AnimalState::Drink | AnimalState::Feed => {
            // Head down biting; deer tear the grass with a short pull now and then.
            let pull = if deer && st == AnimalState::Graze {
                twitch(k.clock, 2.3, k.seed + 0.4)
            } else {
                0.0
            };
            let tear = if st == AnimalState::Feed {
                twitch(k.clock, 1.6, k.seed)
            } else {
                0.0
            };
            let down = Look {
                neck: if deer { -1.65 } else { -1.9 } + 0.05 * pull + 0.12 * tear,
                neck2: if deer { -0.3 } else { 0.0 } + 0.1 * pull,
                head: if deer { 1.22 } else { 0.9 } + 0.03 * chew - 0.08 * pull - 0.2 * tear,
                stoop: if deer { 0.05 } else { 0.12 },
                yaw: 0.12 * drift(k.clock, 0.5, k.seed),
                ear_up: -0.15,
                jaw: if st == AnimalState::Feed {
                    -0.25 - 0.2 * chew
                } else {
                    0.0
                },
                tail_swish: calm.tail_swish,
                ..Look::default()
            }
            .ear_fwd(0.4 * twitch(k.clock, 2.3, k.seed) - 0.1);
            let up = Look {
                neck: 0.12,
                neck2: 0.12,
                head: 0.42 + 0.012 * chew,
                yaw: 0.3 * drift(k.clock, 0.35, k.seed),
                tail_swish: calm.tail_swish,
                ..Look::default()
            }
            .ear_fwd(0.3);
            // Graze while standing or stepping slowly; head up to walk.
            Some((down.mix(up, watch).mix(l, smooth(0.5, 1.0, k.speed)), 0.7))
        }
        AnimalState::Alert => {
            // Head thrown up toward it and frozen, then scanning; an alarmed boar champs its
            // jaws.
            let frozen = 1.0 - smooth(2.0 + 1.5 * k.vary, 3.0 + 1.5 * k.vary, k.t);
            let scan = 0.35 * drift(k.clock, 0.6, k.seed) * (1.0 - frozen);
            let flag = if deer {
                0.55 + 0.25 * twitch(k.clock, 1.1, k.seed)
            } else {
                0.7
            };
            let champ = if boar && alarmed {
                -0.08 * (k.clock * 7.0).sin().abs()
            } else {
                0.0
            };
            // A boar scents for what it can't see: the snout comes up again and again, the
            // nose working at the air.
            let scent = if boar {
                let c = (k.clock / 1.7 + k.seed * 5.3).fract();
                smooth(0.0, 0.2, c) * (1.0 - smooth(0.55, 0.85, c))
            } else {
                0.0
            };
            let nose = scent * (0.24 + 0.05 * (k.clock * 11.0).sin().max(0.0));
            Some((
                Look {
                    neck: beast.by(0.3, 0.12, 0.2) + 0.05 * scent,
                    neck2: if deer { 0.18 } else { 0.0 },
                    head: beast.by(0.5, 0.02, 0.15) + nose,
                    yaw: scan,
                    ear_up: if boar { 0.4 } else { 0.35 },
                    tail: match beast {
                        Beast::Deer if !alarmed => 0.3,
                        Beast::Boar => 0.5,
                        _ => flag,
                    },
                    jaw: champ,
                    tail_swish: if boar { 0.5 * calm.tail_swish } else { 0.0 },
                    // An alarmed boar's crest stands up.
                    bristle: if alarmed { 1.0 } else { 0.5 },
                    ..Look::default()
                }
                .ear_fwd(if boar { 0.8 } else { 0.7 }),
                0.25,
            ))
        }
        AnimalState::Flee if boar => Some((
            // A running boar carries its head level and its tail up, flicking.
            Look {
                neck: -0.05,
                head: 0.05,
                ear_up: -0.1,
                tail: 0.7,
                tail_swish: 0.25 * (k.clock * TAU * 2.6).sin(),
                bristle: 0.6,
                ..Look::default()
            }
            .ear_fwd(-0.5),
            0.2,
        )),
        AnimalState::Charge => Some((
            // Charging: head down, the tusks leading, tail up.
            Look {
                neck: -0.14,
                head: 0.06,
                tail: 0.9,
                bristle: 1.0,
                ..Look::default()
            }
            .ear_fwd(0.35),
            0.15,
        )),
        AnimalState::Flee | AnimalState::Chase => Some((
            Look {
                // A running wolf carries its head low and level, in line with its back.
                neck: if deer { -0.35 } else { -0.35 - 0.25 * gallop },
                neck2: if deer { -0.05 } else { 0.0 },
                head: if deer { 0.45 } else { 0.4 + 0.2 * gallop },
                ear_up: -0.1,
                // A fleeing deer flags its tail up, flashing the pale rump and underside.
                tail: if deer {
                    1.15
                } else if st == AnimalState::Chase {
                    0.95
                } else {
                    -0.25
                },
                tail2: if deer { 0.0 } else { -0.3 },
                ..Look::default()
            }
            .ear_fwd(if deer { -0.6 } else { -0.8 }),
            if deer {
                0.18
            } else {
                0.3 + 0.5 * (1.0 - gallop)
            },
        )),
        AnimalState::Stalk => Some((
            Look {
                neck: -0.4,
                head: 0.35,
                ear_up: 0.2,
                tail: -0.15,
                tail2: 0.1,
                ..Look::default()
            }
            .ear_fwd(0.6),
            0.5,
        )),
        AnimalState::Rest => Some((
            Look {
                neck: beast.by(0.1, -0.1, -0.12),
                neck2: if deer { 0.12 } else { 0.0 },
                // Deer chew the cud lying down; a boar lies with its chin on the ground.
                head: beast.by(0.2 + 0.01 * chew, 0.25, 0.12),
                yaw: 0.4 * drift(k.clock, 0.15, k.seed),
                // Wolves lie with the tail curled round to the side.
                tail: if wolf { 0.8 } else { 0.0 },
                tail2: if wolf { 0.35 } else { 0.0 },
                tail_swish: beast.by(0.0, 0.75, 0.3 * calm.tail_swish),
                ..Look::default()
            }
            .ear_fwd(0.2 * drift(k.clock, 0.3, k.seed)),
            1.2,
        )),
        AnimalState::Dead => {
            // The head drops as the legs give, the neck lags as the body goes over, then the
            // head hits the ground, bounces once and lies, the jaw falling slack.
            use Ease::{Both, In, Out};
            let t = k.t;
            Some((
                Look {
                    neck: track(
                        t,
                        &[
                            (0.0, -0.1, Both),
                            (0.3, -0.5, Both),
                            (0.6, -0.15, Both),
                            (0.9, -0.28, Both),
                        ],
                    ),
                    neck2: -0.35,
                    head: track(
                        t,
                        &[
                            (0.0, 0.1, Both),
                            (0.3, 0.45, Both),
                            (0.75, -0.25, Both),
                            (0.9, -0.2, Both),
                        ],
                    ),
                    yaw: k.side
                        * track(
                            t,
                            &[
                                (0.0, 0.0, Both),
                                (0.45, -0.3, Both),
                                (0.76, 1.0, In),
                                (0.84, 0.8, Out),
                                (0.95, 0.9, Both),
                            ],
                        ),
                    ear_up: -0.7,
                    tail: -0.2,
                    jaw: track(t, &[(0.6, 0.0, Both), (1.0, -0.12, Both)]),
                    ..Look::default()
                }
                .ear_fwd(-0.3),
                0.05,
            ))
        }
        _ => None,
    };
    if let Some((t, secs)) = target {
        l = l.mix(t, ease(secs));
    }
    // An alarmed deer that isn't running flags its tail.
    if deer && alarmed && st != AnimalState::Flee && st != AnimalState::Dead {
        l.tail = l.tail.max(0.55);
    }
    if st != AnimalState::Stalk {
        l.neck -= 0.25 * crouch;
        l.head += 0.2 * crouch;
    }
    // Its last bite or slash: when it began, and whether it was at it last step.
    if i.biting && !m.biting {
        m.bit = Some(k.now);
    }
    m.biting = i.biting;
    let since = m.bit.map_or(f32::MAX, |t| (k.now - t) as f32);
    if wolf {
        // Closing on its prey a wolf reaches for it, jaws open and ears back; on the bite the
        // head thrusts in at full gape and the jaws snap shut, grip and shake, the head
        // tugging back, then let go.
        let gripping = smooth(0.0, 0.04, since) * (1.0 - smooth(0.55, 0.8, since));
        let reach = match (st, k.prey) {
            (AnimalState::Chase, Some((_, d))) => smooth(3.0, 1.8, d),
            _ => 0.0,
        } * (1.0 - gripping);
        l.neck += 0.25 * reach;
        l.head -= 0.1 * reach;
        l.jaw = l.jaw.min(-0.5 * reach);
        l.ear_up -= 0.2 * reach;
        if since < 0.8 {
            use Ease::{Both, In, Out};
            let jaw = track(
                since,
                &[
                    (0.0, -0.6, Both),
                    (0.05, -0.65, Out),
                    (0.1, 0.0, In),
                    (0.5, 0.0, Both),
                    (0.62, -0.3, Out),
                    (0.8, l.jaw, Both),
                ],
            );
            let thrust = smooth(0.0, 0.05, since) * (1.0 - smooth(0.1, 0.3, since));
            let shake = 0.3
                * ((since - 0.1) * TAU * 7.0).sin()
                * smooth(0.1, 0.16, since)
                * (1.0 - smooth(0.34, 0.54, since));
            let tug = smooth(0.1, 0.2, since) * (1.0 - smooth(0.44, 0.7, since));
            l.jaw = jaw;
            l.gaze += shake;
            l.neck += 0.2 * thrust + 0.15 * tug;
            l.head -= 0.1 * thrust + 0.12 * tug;
            l.ears = [-0.8; 2];
        }
    } else if boar && since < 0.6 {
        // The slash: the head dips under what it strikes and hooks up and sideways through
        // it, mouth open and ears back, then comes back down.
        use Ease::{Both, Out};
        let hook = track(
            since,
            &[
                (0.0, 0.0, Both),
                (0.06, -0.35, Out),
                (0.16, 1.0, Out),
                (0.45, 0.0, Both),
            ],
        );
        // Toward the side it is on.
        let side = k.threat.map_or(1.0, |b| if b > 0.0 { -1.0 } else { 1.0 });
        let open = smooth(0.0, 0.06, since) * (1.0 - smooth(0.3, 0.5, since));
        l.head += 0.55 * hook;
        l.neck += 0.25 * hook;
        l.gaze += side * 0.35 * hook.max(0.0);
        l.jaw = l.jaw.min(-0.2 * open);
        for e in &mut l.ears {
            *e = lerp(*e, -0.8, open);
        }
    }
    // Head round toward what it watches (the body turns on after it); a hunting wolf keeps
    // its eyes on its prey, and a charging boar on what it charges.
    let dead = st == AnimalState::Dead;
    let att = attention(k, m, i).filter(|_| !dead && st != AnimalState::Flee);
    let prey_to = k
        .prey
        .filter(|_| wolf && matches!(st, AnimalState::Stalk | AnimalState::Chase))
        .or(k
            .threat
            .map(|b| (b, 0.0))
            .filter(|_| st == AnimalState::Charge))
        .map(|(b, _)| -b.clamp(-0.9, 0.9));
    let head_to = prey_to.unwrap_or(match att {
        Some((b, w))
            if matches!(
                st,
                AnimalState::Alert
                    | AnimalState::Graze
                    | AnimalState::Wander
                    | AnimalState::Rest
                    | AnimalState::Drink
            ) =>
        {
            -b.clamp(-1.3, 1.3) * w
        }
        _ => 0.0,
    });
    // The head turns to it first (the eyes are not parts of their own), and the neck
    // follows on, the head straightening on it as it comes round.
    if k.fresh {
        m.look = head_to;
        m.gaze = 0.0;
    } else {
        ease_to(
            &mut m.look,
            head_to,
            if st == AnimalState::Alert { 0.22 } else { 0.35 },
            k.dt,
        );
        ease_to(&mut m.gaze, (head_to - m.look).clamp(-0.6, 0.6), 0.05, k.dt);
    }
    l.yaw += m.look / 1.05;
    l.gaze += m.gaze;
    // Up or down to a threat above or below it.
    if st == AnimalState::Alert && k.threat.is_some() {
        l.head += 0.8 * k.threat_up;
    }
    // Ears: each swivels on its own, toward a noise or the threat, and flicks.
    let cup = beast.by(0.37, 0.3, 0.32);
    for s in 0..2 {
        let side = if s == 0 { -1.0 } else { 1.0 };
        let scan = 0.45 * glance(k.clock, 1.7 + 0.6 * s as f32, k.vary + s as f32 * 0.31);
        let calm_ear = l.ears[s] + scan * (1.0 - moving) * if dead { 0.0 } else { 1.0 };
        let want = match att {
            Some((b, w)) => {
                let rel = b + 1.05 * l.yaw + l.gaze;
                lerp(calm_ear, (cup - side * rel).clamp(-2.1, 0.9), w)
            }
            None => calm_ear,
        };
        if k.fresh || dead {
            m.ears[s] = want;
        } else {
            ease_to(&mut m.ears[s], want, 0.07, k.dt);
        }
        let flick = 0.5 * twitch(k.clock, 2.7, k.seed + s as f32 * 0.37);
        l.ears[s] =
            m.ears[s] + flick * if s == 0 { 1.0 } else { -0.6 } * if dead { 0.0 } else { 1.0 };
    }
    // The tail swings after the body through turns and settles.
    m.tail
        .step((-0.1 * m.spin).clamp(-0.4, 0.4) * moving, 1.6, 0.25, k.dt);
    l.tail_swish += m.tail.x;
    if let Some((t0, from)) = m.fade {
        let u = smooth(0.0, 0.35, (k.now - t0) as f32);
        l = from.mix(l, u);
        if u >= 1.0 {
            m.fade = None;
        }
    }
    // What a change of state fades from: the look without the flinch, which is layered on
    // top of whatever it fades to (else it would count twice through the fade).
    m.was = l;
    if let Some((t, from)) = k.flinch {
        // Hurt: the head jerks up and away from the blow, the mouth gapes, the ears go flat
        // back and stay pinned while it gets away, and the tail clamps down.
        use Ease::{Both, Out};
        let jolt = track(
            t,
            &[
                (0.0, 0.0, Both),
                (0.06, 1.0, Out),
                (0.18, 0.6, Both),
                (0.6, 0.0, Both),
            ],
        );
        // The head and neck swing hard away from the blow and hold a moment, so a blow from
        // the far side reads from this side too (the face turned to it).
        let sway = track(
            t,
            &[
                (0.0, 0.0, Both),
                (0.08, 1.0, Out),
                (0.32, 0.85, Both),
                (0.75, 0.0, Both),
            ],
        );
        let pin = smooth(0.0, 0.06, t) * (1.0 - smooth(1.2, 1.8, t));
        let clamp = smooth(0.0, 0.06, t) * (1.0 - smooth(0.4, 0.8, t));
        l.neck += 0.4 * jolt;
        l.head -= 0.2 * jolt;
        l.yaw += from.sin() * 0.85 * sway;
        l.gaze += from.sin() * 0.35 * sway;
        for e in &mut l.ears {
            *e = lerp(*e, -1.6, pin);
        }
        l.ear_up = lerp(l.ear_up, -0.6, pin);
        l.tail = lerp(l.tail, -0.3, clamp);
        l.jaw = l.jaw.min(-0.15 * jolt);
    }
    l
}

/// A rooting boar's neck, head and stoop (rad) with its snout in the soil.
const ROOT: (f32, f32, f32) = (-0.9, 0.8, 0.2);

impl Quad<'_> {
    fn place(&self, out: &mut Pose) {
        out.set(Piece::Body, self.body.placed());
        out.set(Piece::Croup, self.croup().placed());
        for k in 0..4u8 {
            let segs = self.leg(k);
            for (s, p) in segs.iter().enumerate() {
                out.set(Piece::Leg(k, s as u8), *p);
            }
            out.set(Piece::Hoof(k), segs[3]);
        }
        out.set(Piece::Neck, self.neck(0).placed());
        out.set(Piece::Neck2, self.neck(1).placed());
        let head = self.head();
        // A boar's upper tusks stay in its upper lip; the lower ones are its jaw's.
        for p in [Piece::Head, Piece::Gloss, Piece::Antlers, Piece::UpperTusks] {
            out.set(p, head.placed());
        }
        let jaw = head
            .child(self.rig.jaw, Quat::from_rotation_x(self.look.jaw))
            .placed();
        out.set(Piece::Jaw, jaw);
        out.set(Piece::Tusks, jaw);
        for s in 0..2 {
            out.set(Piece::Ear(s), self.ear(s).placed());
        }
        out.set(Piece::Tail, self.tail(0).placed());
        out.set(Piece::Tail2, self.tail(1).placed());
        // A boar's crests ride on its neck and withers, sliding up out of the coat as they
        // rise.
        if self.beast == Beast::Boar {
            let lift = Vec3::Y * self.look.bristle.clamp(0.0, 1.2) * MANE_LIFT;
            let neck = self.neck(0);
            out.set(
                Piece::Mane(0),
                at(neck.point(self.rig.neck + lift), neck.rot),
            );
            out.set(
                Piece::Mane(1),
                at(self.body.point(self.rig.body + lift), self.body.rot),
            );
        }
    }

    fn neck(&self, seg: u8) -> Frame {
        let l = &self.look;
        // The neck follows the spine through the gallop (a deer's stretching out as it
        // extends, a wolf's dipping as it gathers).
        let n1 = self.body.child(
            self.rig.neck,
            Quat::from_rotation_y(l.yaw * 0.5)
                * Quat::from_rotation_x(l.neck + self.b.nod * self.flex),
        );
        if seg == 0 {
            return n1;
        }
        n1.child(
            self.rig.neck2,
            Quat::from_rotation_y(l.yaw * 0.3) * Quat::from_rotation_x(l.neck2),
        )
    }

    fn head(&self) -> Frame {
        let l = &self.look;
        let roll = if self.dead > 0.0 { 0.0 } else { 0.08 * l.yaw };
        self.neck(1).child(
            self.rig.head,
            Quat::from_rotation_y(l.yaw * 0.25 + l.gaze)
                * Quat::from_rotation_x(l.head - self.b.nod * self.flex)
                * Quat::from_rotation_z(roll),
        )
    }

    fn ear(&self, s: u8) -> Frame {
        let l = &self.look;
        let (up, fwd) = match self.beast {
            Beast::Deer => (art::DEER_HEAD_UP, art::DEER_HEAD_FWD),
            Beast::Wolf => (Vec3::Y, Vec3::NEG_Z),
            Beast::Boar => (art::BOAR_HEAD_UP, art::BOAR_HEAD_FWD),
        };
        let side = if s == 0 { -1.0 } else { 1.0 };
        let swivel = l.ears[s as usize];
        let droop = -l.ear_up;
        let rot =
            Quat::from_axis_angle(up, side * swivel) * Quat::from_axis_angle(fwd, side * droop);
        let pivot = if s == 0 {
            flip(self.rig.ear)
        } else {
            self.rig.ear
        };
        self.head().child(pivot, rot)
    }

    /// The croup, turned about the loin: tucked under as the spine flexes (+), raised behind
    /// as it extends.
    fn croup(&self) -> Frame {
        self.body
            .child(self.rig.loin, Quat::from_rotation_x(self.flex))
    }

    fn tail(&self, seg: u8) -> Frame {
        let l = &self.look;
        // The tail rides on the croup.
        let t1 = self.croup().child(
            self.rig.tail,
            Quat::from_rotation_y(l.tail_swish) * Quat::from_rotation_x(-l.tail),
        );
        if seg == 0 {
            return t1;
        }
        let sway = if self.beast == Beast::Wolf {
            0.12 * (self.plants[2].u * TAU).sin() * self.upright * self.b.flex
        } else {
            0.0
        };
        t1.child(
            self.rig.tail2,
            Quat::from_rotation_y(sway + l.tail_swish) * Quat::from_rotation_x(-l.tail2),
        )
    }

    /// Where the shoulder rides on the shoulder blade, or the hip on the croup, for a foot at
    /// `toe` (rest coordinates).
    fn root(&self, k: u8, toe: Vec3) -> Vec3 {
        let j = self.rig.leg(k);
        if k < 2 {
            let along = (0.35 * (toe.z - j[4].z)).clamp(-self.b.blade, self.b.blade);
            j[0] + Vec3::new(0.0, -0.25 * along.abs(), along)
        } else {
            // The hip rides on the croup: down and forward under the body as it tucks.
            let loin = self.rig.loin;
            loin + Quat::from_rotation_x(self.flex) * (j[0] - loin)
        }
    }

    /// The toe of leg `k` in rest coordinates.
    fn toe(&self, k: u8) -> Vec3 {
        let pl = self.plants[k as usize];
        self.body.rest(Vec3::new(pl.at.x, pl.lift, pl.at.y))
    }

    /// How far the body must come down at the front and back for the planted feet to reach.
    fn reach_drop(&self) -> (f32, f32) {
        let mut drop = [0.0f32; 2];
        for k in 0..4u8 {
            let pl = &self.plants[k as usize];
            if !pl.down && pl.u > 0.35 && pl.u < 0.55 {
                continue;
            }
            let j = self.rig.leg(k);
            let toe = self.toe(k);
            let root = self.root(k, toe);
            let (_, len) = chain::<4>(&j);
            let reach = (len.iter().sum::<f32>()) * 0.985;
            let d = toe - root;
            let flat = Vec2::new(d.x, d.z).length();
            if flat < reach {
                let need = -d.y - (reach * reach - flat * flat).sqrt();
                let e = usize::from(k >= 2);
                drop[e] = drop[e].max(reach_need(pl, need, reach));
            }
        }
        (drop[0], drop[1])
    }

    fn leg(&self, k: u8) -> [Placed; 4] {
        let rig = self.rig;
        let j = rig.leg(k);
        let front = k < 2;
        let (rest, len) = chain::<4>(&j);
        let mut ang = rest;
        let mut root = j[0];
        let mut abd = 0.0;
        if self.upright > 0.0 {
            let toe = self.toe(k);
            root = self.root(k, toe);
            let (foot_d, flex) = self.style[k as usize];
            let s = solve_quad_leg(&j, root, toe, foot_d, flex, front);
            ang = s.ang;
            abd = s.abduct * self.upright;
            root = j[0] + (s.root - j[0]) * self.upright;
        }
        let fold = self.fold(front);
        let w_fold = smooth(0.0, 0.7, self.lie);
        for i in 0..4 {
            ang[i] += wrap_pi(fold[i] - ang[i]) * w_fold;
        }
        if self.dead > 0.0 {
            // Limp: knees and hocks a little bent, each leg a little differently.
            let vary = 0.18 * (k as f32 * 1.7 + self.seed * 11.0).sin();
            let limp = if front {
                [
                    rest[0] + 0.3 + vary,
                    rest[1] + 0.1,
                    rest[2] - 0.55 - vary,
                    rest[3] - 0.35,
                ]
            } else {
                [
                    rest[0] + 0.25 + vary,
                    rest[1] + 0.05,
                    rest[2] + 0.45 - vary,
                    rest[3] - 0.35,
                ]
            };
            for i in 0..4 {
                ang[i] += wrap_pi(limp[i] - ang[i]) * self.dead;
            }
        }
        let side = if k.is_multiple_of(2) { -1.0 } else { 1.0 };
        let splay = match (front, self.beast) {
            (true, _) => 0.08,
            (false, Beast::Deer) => 0.5,
            (false, Beast::Wolf) => 0.32,
            (false, Beast::Boar) => 0.3,
        } * w_fold;
        // Dead, the limp legs flop toward the ground as the body lands (the upper ones over
        // the lower), kicked up off it as it hits, and settle.
        let droop = self.fall.map_or(0.0, |(t, over)| {
            use Ease::{Both, In, Out};
            let rest = if (side > 0.0) == (over > 0.0) {
                0.32
            } else {
                0.1
            };
            -over
                * track(
                    t,
                    &[
                        (0.49, 0.0, Both),
                        (0.68, -0.2, Out),
                        (0.86, rest + 0.06, In),
                        (0.99, rest, Both),
                    ],
                )
        });
        let rab = Quat::from_rotation_z(side * splay + abd + droop);
        // Forward kinematics from the shoulder or hip.
        let mut p = root;
        std::array::from_fn(|i| {
            let here = root + rab * (p - root);
            let rot = rab * Quat::from_rotation_x(ang[i] - rest[i]);
            let d = dir(ang[i]) * len[i];
            p += Vec3::new(j[i + 1].x - j[i].x, d.x, d.y);
            Frame {
                origin: rig.body,
                pos: self.body.point(here),
                rot: self.body.rot * rot,
            }
            .placed()
        })
    }

    /// Leg angles lying down: forelegs folded under the chest (a wolf's stretched out in
    /// front), hind legs tucked to the side.
    fn fold(&self, front: bool) -> [f32; 4] {
        match (self.beast, front) {
            (Beast::Deer, true) => [2.6, -1.35, 1.75, 2.6],
            (Beast::Deer, false) => [-2.4, 1.9, -1.65, -1.3],
            (Beast::Wolf, true) => [2.75, -1.45, -1.6, -1.55],
            (Beast::Wolf, false) => [-2.0, 1.5, -1.5, -1.5],
            (Beast::Boar, true) => FOLD_BOAR.0,
            (Beast::Boar, false) => FOLD_BOAR.1,
        }
    }
}

/// A boar's leg angles lying down (fore, hind).
const FOLD_BOAR: ([f32; 4], [f32; 4]) = ([2.6, -1.35, 1.75, 2.6], [-2.4, 1.9, -1.65, -1.3]);

// ======================================================================= rabbits

fn rabbit(i: &Input, m: &mut Motion, k: &Tick, out: &mut Pose) {
    let r = &RABBIT;
    let st = i.state;
    // Killed: how long ago, and how limp it has gone.
    let fall = (st == AnimalState::Dead).then_some(k.t);
    let dead = fall.map_or(0.0, |t| smooth(0.0, 0.4, t));
    let v_eff = (k.speed * k.speed + (m.spin * 0.12).powi(2)).sqrt() * (1.0 - dead);
    let moving = smooth(0.03, 0.3, v_eff);
    let fast = smooth(1.5, 5.0, v_eff);
    // Sitting up when alert, nose down grazing, lying low to rest: each eased in and out.
    let on = |yes: bool| if yes { 1.0 } else { 0.0 };
    let want = [
        on(st == AnimalState::Alert) * (1.0 - moving),
        on(matches!(st, AnimalState::Graze | AnimalState::Drink)) * (1.0 - moving),
        on(st == AnimalState::Rest),
    ];
    for (n, (b, secs)) in m.blend.iter_mut().zip([0.35, 0.5, 1.0]).enumerate() {
        if k.fresh {
            *b = want[n] * smooth(0.0, secs, k.t);
        } else {
            ease_to(b, want[n], secs * 0.3, k.dt);
        }
    }
    let [sit_up, graze, rest] = m.blend;
    // Hops: the fore feet come down one after the other, the body vaults over them, the
    // hind feet swing through and land (ahead of where the fore feet were, at speed), and
    // push off. Quicker and longer with speed; the fore feet can only be down as long as
    // their short legs reach (half the stance inside the reach, so a paw never lands out of
    // it), the shoulders sliding and the chest dipping over them.
    let freq = 2.0 + 0.3 * v_eff;
    let duty_of = |most: f32, share: f32| {
        if v_eff > 1e-3 {
            ((v_eff * share / freq).min(most) * freq / v_eff).min(share)
        } else {
            share
        }
    };
    const FORE_REACH: f32 = 0.065;
    let (df, dh) = (
        duty_of((0.085 + 0.025 * fast).min(1.7 * FORE_REACH), 0.35),
        duty_of(0.2, 0.5),
    );
    let home = |n: usize| {
        let j = if n < 2 { r.front } else { r.hind };
        let toe = if n.is_multiple_of(2) {
            flip(j[2])
        } else {
            j[2]
        };
        // At speed the fore feet sweep from under the chin to behind the shoulders.
        let back = if n < 2 { 0.025 * fast } else { 0.0 };
        Vec2::new(toe.x, toe.z + back)
    };
    let p = Pattern {
        offsets: mix_phases([0.0, 0.07, 0.42, 0.45], [0.0, 0.14, 0.4, 0.42], fast),
        duty: [df, df, dh, dh],
        freq,
        stepping: v_eff > 0.03 && dead == 0.0,
        lift: [
            0.022 + 0.008 * fast,
            0.022 + 0.008 * fast,
            0.032 + 0.015 * fast,
            0.032 + 0.015 * fast,
        ],
        home: std::array::from_fn(home),
        // The dead don't step.
        reach: if fall.is_some() {
            [f32::MAX; 4]
        } else {
            [FORE_REACH, FORE_REACH, 0.12, 0.12]
        },
        tidy: if fall.is_some() { f32::MAX } else { 0.015 },
        tidy_secs: 0.18,
    };
    if k.fresh {
        m.cycles = f64::from(i.phase / TAU);
        m.reset_feet(k.now, k.v, &p);
    }
    // Killed on the move, the feet skid on under the body.
    let mut plants = if fall.is_some() {
        m.step_feet(k.now, k.dt, 0.0, Vec2::ZERO, 0.0, &p)
    } else {
        m.step_feet(k.now, k.dt, k.turned, k.v, m.spin, &p)
    };
    // At speed the fore feet reach out ahead of where they come down, and draw back onto it.
    for pl in plants.iter_mut().take(2) {
        if !pl.down {
            pl.at.y -= 0.03 * fast * smooth(0.25, 0.7, pl.u) * (1.0 - smooth(0.75, 1.0, pl.u));
        }
    }
    let mut comp = [0.0f32; 2];
    for (n, pl) in plants.iter().enumerate() {
        if pl.down && p.stepping {
            comp[n / 2] = comp[n / 2].max((PI * pl.u).sin());
        }
    }
    // The body vaults over the fore feet, its haunches rising as the hind feet swing through
    // under it, and rises in front as the fore feet lift and the hind legs push.
    let swing = |n: usize| {
        let pl = plants[n];
        if pl.down || !p.stepping {
            0.0
        } else {
            (PI * pl.u).sin()
        }
    };
    let vault = (0.03 + 0.012 * fast) * moving * 0.5 * (swing(2) + swing(3));
    let rise = (0.01 + 0.008 * fast) * moving * 0.5 * (swing(0) + swing(1));
    let bounce = 0.012 * moving;
    let dy_f = -bounce * comp[0] + rise;
    let dy_h = -bounce * comp[1] + vault;
    let arc = m.flight_arc(k.now, 1.0);
    let a_fwd = -m.accel.y;
    m.lean.step(
        (0.02 * a_fwd).clamp(-0.15, 0.15) * (1.0 - dead),
        2.0,
        0.4,
        k.dt,
    );
    let lz = r.hind[0].z - r.front[0].z;
    // Body: hops, rocks over the fore and hind feet, sits up on its haunches, flattens to
    // rest.
    let mut pos = r.body + Vec3::Y * ((dy_f + dy_h) * 0.5 + arc - 0.018 * rest - 0.008 * graze);
    let pitch = (dy_f - dy_h) / lz + m.lean.x + 0.95 * sit_up + 0.08 * graze;
    pos += Vec3::new(0.0, 0.05, 0.035) * sit_up;
    if k.fresh {
        m.breath = k.seed;
    }
    let exert = ((1.0 - i.stamina) * 1.6).clamp(0.0, 1.0);
    m.breath = (m.breath + lerp(0.7, 2.6, exert) * k.dt).fract();
    pos.y += (0.0015 + 0.002 * exert) * (TAU * m.breath).sin();
    let mut rot = Quat::from_rotation_x(pitch);
    // Hurt: jolted away from the blow, rolling off it, ears flat.
    let pin = k.flinch.map_or(0.0, |(t, _)| {
        smooth(0.0, 0.05, t) * (1.0 - smooth(1.0, 1.5, t))
    });
    if let Some((t, from)) = k.flinch {
        use Ease::{Both, Out};
        let jolt = track(
            t,
            &[
                (0.0, 0.0, Both),
                (0.05, 1.0, Out),
                (0.14, 0.7, Both),
                (0.5, 0.0, Both),
            ],
        );
        pos += Vec3::new(-from.sin(), 0.0, from.cos()) * 0.03 * jolt;
        rot = Quat::from_rotation_z(from.sin() * 0.15 * jolt) * rot;
    }
    if let Some(t) = fall {
        // Killed: it slumps, rolls onto its side, bounces once and lies still.
        use Ease::{Both, In, Out};
        pos.y = track(
            t,
            &[
                (0.0, pos.y, Both),
                (0.32, 0.045, In),
                (0.39, 0.056, Out),
                (0.48, 0.045, In),
            ],
        );
        let roll = k.side
            * track(
                t,
                &[
                    (0.0, 0.0, Both),
                    (0.1, 0.06, Both),
                    (0.32, FRAC_PI_2, In),
                    (0.4, FRAC_PI_2 - 0.15, Out),
                    (0.5, FRAC_PI_2 + 0.04, Both),
                    (0.6, FRAC_PI_2, Both),
                ],
            );
        let nose = track(
            t,
            &[
                (0.0, pitch, Both),
                (0.2, pitch - 0.15, Both),
                (0.45, 0.0, Both),
            ],
        );
        rot = Quat::from_rotation_z(roll) * Quat::from_rotation_x(nose);
    }
    let mut body = Frame {
        origin: r.body,
        pos,
        rot,
    };
    let toe_of = |body: &Frame, n: usize| {
        let pl = plants[n];
        body.rest(Vec3::new(pl.at.x, pl.lift, pl.at.y))
    };
    let joints = |n: usize| {
        let j = if n < 2 { r.front } else { r.hind };
        if n.is_multiple_of(2) { j.map(flip) } else { j }
    };
    let root_of = |n: usize, toe: Vec3| {
        let j = joints(n);
        if n < 2 {
            let along = (0.5 * (toe.z - j[2].z)).clamp(-0.045, 0.05);
            j[0] + Vec3::new(0.0, 0.0, along)
        } else {
            j[0]
        }
    };
    // Down as far as the planted feet need.
    if dead == 0.0 {
        for _ in 0..2 {
            let mut drop = [0.0f32; 2];
            for (n, pl) in plants.iter().enumerate() {
                if (!pl.down && pl.u > 0.35 && pl.u < 0.55) || (n < 2 && sit_up > 0.0) {
                    continue;
                }
                let j = joints(n);
                let reach = ((j[1] - j[0]).length() + (j[2] - j[1]).length()) * 0.985;
                let toe = toe_of(&body, n);
                let d = toe - root_of(n, toe);
                let flat = Vec2::new(d.x, d.z).length();
                if flat < reach {
                    let need = -d.y - (reach * reach - flat * flat).sqrt();
                    // A fore paw peeling off at a run is still as far back as its short leg
                    // goes: the frame it lifts, the chest stays down over it (unless the other
                    // paw is coming down) so it leaves the ground where it stood.
                    let other = plants[n ^ 1];
                    let landing = !other.down && other.u > 0.6;
                    let need = if n < 2 && !pl.down && pl.u < 0.05 && !landing {
                        need.min(0.25 * reach)
                    } else {
                        reach_need(pl, need, reach)
                    };
                    drop[n / 2] = drop[n / 2].max(need);
                }
            }
            if drop[0] <= 0.0 && drop[1] <= 0.0 {
                break;
            }
            body.pos.y -= (drop[0] + drop[1]) * 0.5;
            body.rot *= Quat::from_rotation_x(-(drop[0] - drop[1]) / lz);
        }
    }
    out.set(Piece::Body, body.placed());
    // Head and ears.
    let nibble = (k.clock * 9.0).sin() * 0.04 * graze;
    let att = attention(k, m, i).filter(|_| dead == 0.0);
    let look_to = match att {
        Some((b, w)) if st != AnimalState::Flee => -b.clamp(-1.0, 1.0) * w,
        _ => 0.4 * drift(k.clock, 0.5, k.seed) * (1.0 - moving),
    };
    if k.fresh {
        m.look = look_to;
    } else {
        ease_to(&mut m.look, look_to, 0.1, k.dt);
    }
    // The head keeps steady through the rocking of the hops.
    let tilt = (body.rot * Vec3::NEG_Z).y.asin();
    let head_pitch = -0.55 * graze + nibble - 0.85 * sit_up + 0.2 * moving * (1.0 - fast)
        - 0.15 * fast
        + 0.1 * rest
        - 0.8 * (tilt - 0.95 * sit_up - 0.08 * graze) * (1.0 - dead);
    // Dead, the head falls to the ground after the body, bouncing once.
    let loll = fall.map_or(0.0, |t| {
        use Ease::{Both, In, Out};
        k.side
            * track(
                t,
                &[
                    (0.25, 0.0, Both),
                    (0.42, 0.55, In),
                    (0.5, 0.42, Out),
                    (0.6, 0.5, Both),
                ],
            )
    });
    let head = body.child(
        r.head,
        Quat::from_rotation_y(m.look + loll) * Quat::from_rotation_x(head_pitch),
    );
    out.set(Piece::Head, head.placed());
    out.set(Piece::Gloss, head.placed());
    for s in 0..2u8 {
        let side = if s == 0 { -1.0 } else { 1.0 };
        let flat = rest.max(fast * 0.8).max(dead).max(pin);
        let back =
            0.25 + 0.9 * flat - 0.2 * sit_up + 0.15 * twitch(k.clock, 1.9, k.seed + s as f32 * 0.4);
        let spread = 0.12 + 0.15 * graze - 0.08 * sit_up;
        // Each ear turns on its own toward what it hears.
        let scan = 0.35 * glance(k.clock, 1.3 + 0.5 * s as f32, k.vary + s as f32 * 0.29);
        let want = match att {
            Some((b, w)) => lerp(scan, -(b + m.look).clamp(-1.4, 1.4), w),
            None => scan,
        } * (1.0 - flat);
        if k.fresh || dead > 0.0 {
            m.ears[s as usize] = want;
        } else {
            ease_to(&mut m.ears[s as usize], want, 0.06, k.dt);
        }
        let pivot = if s == 0 { flip(r.ear) } else { r.ear };
        out.set(
            Piece::Ear(s),
            head.child(
                pivot,
                Quat::from_rotation_y(m.ears[s as usize])
                    * Quat::from_rotation_z(-side * spread)
                    * Quat::from_rotation_x(back),
            )
            .placed(),
        );
    }
    let up = 0.9 * smooth(1.0, 3.0, k.speed) + 0.4 * sit_up;
    out.set(
        Piece::Tail,
        body.child(r.tail, Quat::from_rotation_x(-up)).placed(),
    );
    // Legs: solved to the planted feet; the fore legs dangle sitting up.
    for (n, pl) in plants.iter().enumerate() {
        let front = n < 2;
        let j = joints(n);
        let (rest_a, len) = chain::<2>(&j);
        let mut toe = toe_of(&body, n);
        let root = root_of(n, toe);
        if !pl.down {
            // Once clear of the ground a swinging leg keeps its joint bent (a foot further out
            // than that hangs short), and a hind foot folds up under the belly, heel up and
            // toes forward, rather than trailing out behind on a straight leg.
            let clear = smooth(0.006, 0.02, pl.lift);
            if !front {
                let fold = j[2] + Vec3::new(0.0, 0.03, -0.005);
                let w = smooth(0.02, 0.15, pl.u) * (1.0 - smooth(0.6, 0.9, pl.u));
                toe = toe.lerp(fold, w * clear * moving);
            }
            let most = (len[0] + len[1]) * if front { 0.92 } else { 0.8 };
            let hang = root + (toe - root).clamp_length_max(most);
            toe = toe.lerp(hang, clear);
        }
        let (abd, t) = abduct(root, toe, j[2].x - root.x);
        let bend = {
            let a = yz(j[1]) - yz(j[0]);
            let b = yz(j[2]) - yz(j[1]);
            -(a.x * b.y - a.y * b.x).signum()
        };
        let (a0, a1) = two_bone(yz(root), t, len[0], len[1], bend);
        let mut upper = a0 - rest_a[0];
        let mut lower = a1 - rest_a[1] - upper;
        if front {
            upper += (-0.9 - upper) * sit_up;
            lower += (0.6 - lower) * sit_up;
        }
        if dead > 0.0 {
            upper += (if front { -0.5 } else { 0.6 } - upper) * dead;
            lower *= 1.0 - dead;
        }
        let abd = abd * (1.0 - dead) * if front { 1.0 - sit_up } else { 1.0 };
        // The leg hangs from where the shoulder has slid to.
        let rab = Frame {
            origin: j[0],
            pos: body.point(root),
            rot: body.rot * Quat::from_rotation_z(abd),
        };
        let a = rab.child(j[0], Quat::from_rotation_x(upper));
        out.set(Piece::Leg(n as u8, 0), a.placed());
        out.set(
            Piece::Leg(n as u8, 1),
            a.child(j[1], Quat::from_rotation_x(lower)).placed(),
        );
    }
}

// ======================================================================= birds

/// Wing beat, the hand folded back on the upstroke: elevation, sweep back, wrist fold and hand
/// lift (rad) at beat phase `f` (rad). The wings are at the top at π/2 and the downstroke takes
/// most of the beat, so they are near the bottom at 3π/2.
fn stroke(f: f32, amp: f32) -> (f32, f32, f32, f32) {
    let c = ((f - FRAC_PI_2) / TAU).rem_euclid(1.0);
    const DOWN: f32 = 0.58;
    if c < DOWN {
        let d = c / DOWN;
        let e = (PI * d).cos();
        (0.15 + amp * e, -0.08 + 0.1 * d, 0.1, 0.12 * (PI * d).sin())
    } else {
        let w = (c - DOWN) / (1.0 - DOWN);
        let e = -(PI * w).cos();
        let s = (PI * w).sin();
        (0.15 + amp * e, 0.02 + 0.3 * s, 0.15 + 0.7 * s, -0.2 * s)
    }
}

fn bird(i: &Input, m: &mut Motion, k: &Tick, out: &mut Pose) {
    let b = &BIRD;
    let st = i.state;
    let flying = st == AnimalState::Flying;
    let dead = st == AnimalState::Dead;
    let fl = i.bird.unwrap_or_default();
    let vel = if fl.velocity.length_squared() > 0.0 {
        fl.velocity
    } else {
        i.vel
    };
    let vy = vel.y;
    let speed = vel.length();
    // The roll the flight code gave the body (taken out, and the bank into the turn put in).
    let (_, ai_pitch, ai_roll) = i.facing.to_euler(EulerRot::YXZ);
    if k.fresh {
        m.wing = fl.flap;
    } else if m.state != Some(st) && flying {
        // Off the ground with the wings sweeping up for a full first beat from the top.
        m.wing = FRAC_PI_2 - 0.3 * TAU;
        m.beats = 0;
    }
    m.state = Some(st);
    // Takeoff: legs push and the wings sweep wide; landing: the wings flare.
    let takeoff = if flying {
        1.0 - smooth(0.25, 0.55, k.t)
    } else {
        0.0
    };
    let to_land = fl
        .landing
        .filter(|_| flying)
        .map(|p| (p - i.at).length() as f32);
    let flare_to = to_land.map_or(0.0, |d| smooth(1.6, 0.35, d));
    let settle = if st == AnimalState::Perched {
        1.0 - smooth(0.0, 0.35, k.t)
    } else {
        0.0
    };
    let glide_to = if flying && takeoff < 0.1 && flare_to < 0.2 {
        smooth(-0.3, -0.9, vy).max(if to_land.is_some() { 0.8 } else { 0.0 })
    } else {
        0.0
    };
    if k.fresh {
        m.flare = flare_to;
        m.glide = glide_to;
    } else {
        ease_to(&mut m.flare, flare_to, 0.08, k.dt);
        ease_to(&mut m.glide, glide_to, 0.12, k.dt);
    }
    // Bounding: in level flight, a few beats, then the wings fold for a moment.
    let level = flying && takeoff == 0.0 && to_land.is_none() && vy.abs() < 0.8 && k.t > 1.2;
    let bounding = m.bound.and_then(|(t0, dur)| {
        let u = (k.now - t0) as f32 / dur;
        (u < 1.0).then_some(u)
    });
    let rate = if flying {
        TAU * (7.0 + 2.0 * takeoff.max(smooth(0.5, 2.0, vy)) + 3.0 * m.flare)
    } else {
        0.0
    };
    if bounding.is_none() && flying {
        let before = ((m.wing - FRAC_PI_2) / TAU).floor();
        m.wing += rate * k.dt;
        let after = ((m.wing - FRAC_PI_2) / TAU).floor();
        if after > before {
            m.beats += 1;
        }
        // Fold at the bottom of a downstroke.
        let c = ((m.wing - FRAC_PI_2) / TAU).rem_euclid(1.0);
        let need = 3 + ((k.vary * 7.0 + k.clock * 0.37).sin().abs() * 3.0) as u32;
        if level && m.beats >= need && (0.5..0.62).contains(&c) {
            m.bound = Some((k.now, 0.16 + 0.1 * k.vary));
            m.beats = 0;
        }
        m.wing = m.wing.rem_euclid(TAU * 64.0);
    }
    if !flying {
        m.bound = None;
        m.beats = 0;
    }
    let fold = bounding.map_or(0.0, |u| smooth(0.0, 0.25, u) * (1.0 - smooth(0.75, 1.0, u)));
    // Bank into the turn as steeply as the turn takes (the lift tilted to pull it round).
    let bank_to = if flying {
        0.9 * (speed * m.spin / 9.81).atan()
    } else {
        0.0
    };
    if k.fresh {
        m.bank = bank_to;
    } else {
        ease_to(&mut m.bank, bank_to, 0.15, k.dt);
    }

    // On the ground: hops lift it (the flight code moves the body), pecks tip it forward.
    let hopping = !flying && vy.abs() > 0.05;
    let peck_t = (k.clock * 1.7 + k.seed * 5.0).fract();
    let pecking = !flying
        && !hopping
        && !dead
        && settle == 0.0
        && (k.clock * 0.31 + k.seed * 3.0).sin() > -0.2;
    let peck = if pecking {
        smooth(0.0, 0.12, peck_t) * (1.0 - smooth(0.2, 0.38, peck_t))
    } else {
        0.0
    };
    let mut pos = b.body;
    // A peck tips the whole body forward to drive the bill into the ground.
    let mut pitch = -0.55 * peck + if hopping { 0.12 } else { 0.0 };
    if flying {
        pos.y += 0.02;
        pitch = 0.2 * takeoff + 0.85 * m.flare;
    }
    let roll = if flying { m.bank - ai_roll } else { 0.0 };
    let mut rot = Quat::from_rotation_z(roll) * Quat::from_rotation_x(pitch);
    if !flying {
        // Pivot about the hips so the feet stay put; a landing sinks into the legs.
        pos.y += b.hip.y
            - (pos + rot * (b.hip - b.body)).y
            - 0.008 * (PI * (1.0 - settle)).sin() * settle.min(1.0);
    }
    // Killed: one shot out of the air tumbles down, limp wings fluttering, rolled half over;
    // on the ground it topples onto its side, bounces once and lies still.
    let falling = dead && speed > 0.05;
    if !dead || falling {
        m.downed = None;
    } else if m.downed.is_none() {
        // It comes down as it was tumbling (level if it was killed on the ground).
        m.downed = Some((k.now, m.tumble.0, m.tumble.1));
    }
    if !dead {
        m.tumble = (0.0, 0.0);
    }
    if dead {
        use Ease::{Both, In, Out};
        let side = k.side;
        if falling {
            pos.y = b.body.y;
            m.tumble = (1.2 * smooth(0.0, 0.4, k.t), -0.6 * smooth(0.0, 0.3, k.t));
            rot = Quat::from_rotation_z(side * m.tumble.0) * Quat::from_rotation_x(m.tumble.1);
        } else if let Some((t0, from, pitch)) = m.downed {
            let t = (k.now - t0) as f32;
            let roll = track(
                t,
                &[
                    (0.0, from, Both),
                    (0.14, FRAC_PI_2 + 0.15, In),
                    (0.22, FRAC_PI_2 - 0.08, Out),
                    (0.32, FRAC_PI_2, Both),
                ],
            );
            pos.y = track(
                t,
                &[
                    (0.0, pos.y, Both),
                    (0.14, 0.03, In),
                    (0.19, 0.036, Out),
                    (0.25, 0.03, In),
                ],
            );
            // The nose comes up level as it topples.
            let pitch = pitch * (1.0 - smooth(0.0, 0.14, t));
            rot = Quat::from_rotation_z(side * roll) * Quat::from_rotation_x(pitch);
        }
    }
    let body = Frame {
        origin: b.body,
        pos,
        rot,
    };
    out.set(Piece::Body, body.placed());
    // The head stays level through banks and pitches, and jerks round between still
    // moments on the ground.
    let turn_head = ((k.clock * 1.3 + k.seed * 11.0).floor() * 2.39).sin() * 0.6;
    let steady = if flying { 0.8 } else { 0.0 };
    let head = body.child(
        b.head,
        Quat::from_rotation_z(-m.bank * steady)
            * Quat::from_rotation_y(if flying || dead { 0.0 } else { turn_head })
            * Quat::from_rotation_x(
                -peck + if flying { 0.25 } else { 0.0 } - steady * 0.6 * (pitch + ai_pitch),
            ),
    );
    out.set(Piece::Head, head.placed());
    out.set(Piece::Gloss, head.placed());
    // Wings.
    let wings_out = flying || settle > 0.0 || falling;
    let folded = settle.max(fold);
    let amp = 0.8 + 0.25 * takeoff - 0.45 * m.flare;
    let (mut elev, mut sweep, mut wrist, mut hand_up) = stroke(m.wing, amp);
    // Gliding: spread flat with a little dihedral.
    let g = m.glide * (1.0 - m.flare);
    elev = lerp(elev, 0.12, g);
    sweep = lerp(sweep, 0.05, g);
    wrist = lerp(wrist, 0.08, g);
    hand_up = lerp(hand_up, 0.05, g);
    // Flaring: spread forward and high, beating shallow and quick.
    sweep -= 0.35 * m.flare;
    elev += 0.35 * m.flare;
    // Takeoff: a wide forward sweep.
    sweep -= 0.15 * takeoff;
    if falling {
        // Limp, half open, fluttering in the rush of air.
        let flutter = (k.clock * 31.0).sin();
        elev = 0.35 + 0.15 * flutter;
        sweep = 0.5;
        wrist = 0.9 + 0.2 * flutter;
        hand_up = 0.1;
    }
    // Folded (a bound, or settling after landing): swept back along the body.
    elev = lerp(elev, -0.08, folded);
    sweep = lerp(sweep, 1.35, folded);
    wrist = lerp(wrist, 2.3, folded);
    hand_up = lerp(hand_up, 0.0, folded);
    for s in 0..2u8 {
        let side = if s == 0 { -1.0 } else { 1.0 };
        let (shoulder, wrist_at) = if s == 0 {
            (flip(b.shoulder), flip(b.wrist))
        } else {
            (b.shoulder, b.wrist)
        };
        // The outer wing of a bank rises a little.
        let lift = elev + 0.1 * m.bank * side * (1.0 - folded);
        let arm = body.child(
            shoulder,
            Quat::from_rotation_z(side * lift) * Quat::from_rotation_y(-side * sweep * 0.5),
        );
        let hand = arm.child(
            wrist_at,
            Quat::from_rotation_y(-side * (wrist + 0.5 * sweep))
                * Quat::from_rotation_z(side * hand_up),
        );
        let show = |p: Placed, on: bool| if on { p } else { HIDDEN };
        out.set(Piece::Wing(s, 0), show(arm.placed(), wings_out));
        out.set(Piece::Wing(s, 1), show(hand.placed(), wings_out));
        out.set(Piece::Wing(s, 2), show(body.placed(), !wings_out));
    }
    // Tail: folded on the ground (twitching), fanned in the air, twisting into turns and
    // spread down to brake when landing.
    let tail_fold = body.child(
        b.tail,
        Quat::from_rotation_x(0.25 * twitch(k.clock, 1.3, k.seed) - 0.3 * peck + 0.3 * settle),
    );
    out.set(
        Piece::Tail,
        if flying && settle == 0.0 {
            HIDDEN
        } else {
            tail_fold.placed()
        },
    );
    let fan = body.child(
        b.tail,
        Quat::from_rotation_z(0.35 * m.bank)
            * Quat::from_rotation_x(
                -0.15 * m.glide + 0.1 * m.wing.sin() * (1.0 - m.glide) - 0.5 * m.flare,
            ),
    );
    out.set(
        Piece::TailFan,
        if flying && settle == 0.0 {
            fan.placed()
        } else {
            HIDDEN
        },
    );
    // Legs: tucked in flight, pushing off at takeoff, reaching forward to land.
    for s in 0..2u8 {
        let hip = if s == 0 { flip(b.hip) } else { b.hip };
        let reach = m
            .flare
            .max(smooth(0.5, 0.9, g) * 0.6 * f32::from(to_land.is_some()));
        let push = takeoff;
        let ang = if flying {
            // Tucked back along the belly; pushing down and back off the ground; reaching
            // down and forward for it.
            let up = pitch + ai_pitch;
            let tucked = -1.35;
            let pushed = -up - 0.35;
            let reached = -up + 0.55;
            let a = lerp(tucked, pushed, push);
            lerp(a, reached, reach)
        } else if hopping {
            0.4
        } else {
            // Standing upright whatever the body does.
            -pitch
        };
        let shown = !flying || push > 0.3 || reach > 0.3;
        out.set(
            Piece::Leg(s, 0),
            if shown {
                body.child(hip, Quat::from_rotation_x(ang)).placed()
            } else {
                HIDDEN
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pose(
        species: Species,
        part: Part,
        state: AnimalState,
        phase: f32,
        speed: f32,
        now: f64,
    ) -> Placed {
        let loco = LocomotionState::default();
        animal_pose(
            species,
            part,
            state,
            now - 5.0,
            now,
            phase,
            speed,
            &loco,
            false,
            0.0,
            false,
        )
    }

    fn rig(species: Species) -> &'static QuadRig {
        match species {
            Species::Deer => &DEER,
            Species::Boar => &BOAR,
            _ => &WOLF,
        }
    }

    /// Where the toe of leg `k` is.
    fn toe(species: Species, k: u8, state: AnimalState, phase: f32, speed: f32, now: f64) -> Vec3 {
        let j = rig(species).leg(k);
        let p = pose(species, Part::Beast(LEG_FOOT + k), state, phase, speed, now);
        p.pos + p.rot * (j[4] - j[3])
    }

    #[test]
    fn standing_animals_stand_on_their_feet() {
        for species in [Species::Deer, Species::Wolf, Species::Boar] {
            for k in 0..4 {
                let t = toe(species, k, AnimalState::Wander, 0.0, 0.0, 100.0);
                assert!(t.y.abs() < 0.01, "{species:?} leg {k}: toe at {t}");
            }
        }
    }

    #[test]
    fn moving_feet_touch_the_ground_and_never_sink() {
        for species in [Species::Deer, Species::Wolf, Species::Boar] {
            for (speed, label) in [(1.2, "walk"), (3.2, "trot"), (9.0, "gallop")] {
                for i in 0..40 {
                    let phase = i as f32 / 40.0 * TAU;
                    let now = 100.0 + i as f64 / 40.0 / 2.2;
                    let feet: Vec<Vec3> = (0..4)
                        .map(|k| toe(species, k, AnimalState::Wander, phase, speed, now))
                        .collect();
                    let low = feet.iter().map(|f| f.y).fold(f32::MAX, f32::min);
                    assert!(low > -0.02, "{species:?} {label} {i}: a toe sinks to {low}");
                    if label == "walk" {
                        assert!(
                            low < 0.01,
                            "{species:?} {label} {i}: no foot on the ground ({low})"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn leg_segments_stay_joined() {
        for species in [Species::Deer, Species::Wolf, Species::Boar] {
            let j = rig(species).leg(1);
            for (state, speed) in [
                (AnimalState::Wander, 1.2),
                (AnimalState::Flee, 9.0),
                (AnimalState::Rest, 0.0),
                (AnimalState::Dead, 0.0),
            ] {
                for k in 0..4u8 {
                    let j = if k == 1 { j } else { rig(species).leg(k) };
                    let segs = [
                        Part::Leg(k),
                        Part::Beast(LEG_MID + k),
                        Part::Beast(LEG_LOW + k),
                        Part::Beast(LEG_FOOT + k),
                    ];
                    for s in 1..4 {
                        let a = pose(species, segs[s - 1], state, 1.0, speed, 100.0);
                        let b = pose(species, segs[s], state, 1.0, speed, 100.0);
                        let end = a.pos + a.rot * (j[s] - j[s - 1]);
                        assert!(
                            end.distance(b.pos) < 1e-3,
                            "{species:?} {state:?} leg {k} joint {s}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn grazing_deer_reach_the_grass_and_alert_deer_hold_their_heads_high() {
        let nose = |state| {
            let p = pose(Species::Deer, Part::Head, state, 0.0, 0.0, 100.0);
            p.pos + p.rot * (art::DEER_HEAD_FWD * 0.39)
        };
        let graze = nose(AnimalState::Graze);
        let alert = nose(AnimalState::Alert);
        let idle = nose(AnimalState::Wander);
        eprintln!("nose: graze {graze} alert {alert} idle {idle}");
        assert!(graze.y > 0.0 && graze.y < 0.08, "grazing nose at {graze}");
        assert!(
            graze.z < -0.5,
            "grazing nose in front of the chest: {graze}"
        );
        assert!(alert.y > idle.y + 0.1, "alert {alert} idle {idle}");
        for k in 0..4 {
            let t = toe(Species::Deer, k, AnimalState::Graze, 0.0, 0.0, 100.0);
            assert!(t.y.abs() < 0.01, "grazing deer leg {k}: toe at {t}");
        }
    }

    #[test]
    fn feeding_wolves_reach_the_kill_and_stalking_wolves_creep_low() {
        let nose = |state, speed| {
            let p = pose(Species::Wolf, Part::Head, state, 0.0, speed, 100.0);
            p.pos + p.rot * (Vec3::NEG_Z * WOLF.head_len)
        };
        let feed = nose(AnimalState::Feed, 0.0);
        let stand = nose(AnimalState::Wander, 0.0);
        eprintln!("wolf nose: feed {feed} stand {stand}");
        assert!(feed.y > 0.0 && feed.y < 0.15, "feeding nose at {feed}");
        let back = |state| {
            pose(Species::Wolf, Part::Body, state, 0.0, 0.0, 100.0)
                .pos
                .y
        };
        assert!(
            back(AnimalState::Stalk) < back(AnimalState::Wander) - 0.08,
            "stalking body should be low"
        );
        for k in 0..4 {
            let t = toe(Species::Wolf, k, AnimalState::Feed, 0.0, 0.0, 100.0);
            assert!(t.y.abs() < 0.01, "feeding wolf leg {k}: toe at {t}");
        }
    }
    #[test]
    fn a_shot_starling_lands_as_it_was_tumbling() {
        // Shot out of the air and falling for a short or a long while, it comes down rolled
        // and pitched as it tumbled, and topples on from there without a jump.
        for fall in [0.25f32, 0.8] {
            let dt = 1.0 / 60.0;
            let mut m = Motion::default();
            let mut turns = Vec::new();
            let mut landed = None;
            for f in 0..120 {
                let t = f as f32 * dt;
                let now = 1.0 + f64::from(t);
                let falling = t < fall;
                if !falling && landed.is_none() {
                    landed = Some(f);
                }
                let velocity = if falling {
                    Vec3::new(0.0, -9.8 * t - 0.5, -1.0)
                } else {
                    Vec3::ZERO
                };
                let input = Input {
                    species: Species::Bird,
                    male: false,
                    young: false,
                    state: AnimalState::Dead,
                    since: 1.0,
                    now,
                    phase: 0.0,
                    at: DVec3::ZERO,
                    facing: Quat::IDENTITY,
                    vel: velocity,
                    crouch: false,
                    biting: false,
                    vary: 0.37,
                    senses: Senses::default(),
                    bird: Some(BirdFlight {
                        velocity,
                        ..BirdFlight::default()
                    }),
                };
                let body = animal(&input, &mut m).place(Part::Body);
                // Roll about the body's length, and the nose's pitch off level.
                let right = body.rot * Vec3::X;
                let fwd = body.rot * Vec3::NEG_Z;
                turns.push((right.y.atan2(right.x), fwd.z.clamp(-1.0, 1.0).acos()));
            }
            let f = landed.expect("landed");
            let (roll, pitch) = (
                (turns[f].0 - turns[f - 1].0).abs(),
                (turns[f].1 - turns[f - 1].1).abs(),
            );
            let tumbled = (turns[f - 1].0.abs(), (PI - turns[f - 1].1).abs());
            eprintln!(
                "fell {fall} s: tumbling rolled {:.2} and pitched {:.2} rad; landing turned it {roll:.3} and {pitch:.3} rad",
                tumbled.0, tumbled.1
            );
            assert!(
                tumbled.0 > 0.5 && tumbled.1 > 0.2,
                "fell {fall} s: hardly tumbled"
            );
            assert!(
                roll < 0.05 && pitch < 0.05,
                "fell {fall} s: landing jumped {roll:.3} rad in roll and {pitch:.3} in pitch"
            );
        }
    }

    #[test]
    fn starlings_perch_on_their_toes_and_spread_their_wings_to_fly() {
        let b = &BIRD;
        let bird_at = |part: Part, state: AnimalState, flap: f32, now: f64| {
            let loco = LocomotionState::default();
            animal_pose(
                Species::Bird,
                part,
                state,
                95.0,
                now,
                0.0,
                0.0,
                &loco,
                false,
                flap,
                false,
            )
        };
        let bird = |part: Part, state: AnimalState, flap: f32| bird_at(part, state, flap, 100.0);
        let perched = AnimalState::Perched;
        let leg = bird(Part::Leg(1), perched, 0.0);
        let foot = leg.pos + leg.rot * (b.foot - b.hip);
        assert!(leg.visible && foot.y.abs() < 0.005, "foot at {foot}");
        assert!(bird(Part::Beast(WING_FOLDED + 1), perched, 0.0).visible);
        assert!(!bird(Part::Wing(1), perched, 0.0).visible);
        assert!(!bird(Part::Beast(TAIL_FAN), perched, 0.0).visible);
        // Pecking drives the bill into the grass while the feet stay put.
        let bill = Vec3::new(0.0, 0.0955, -0.111);
        let mut lowest = f32::MAX;
        for i in 0..200 {
            let now = 100.0 + i as f64 * 0.01;
            let head = bird_at(Part::Head, perched, 0.0, now);
            lowest = lowest.min((head.pos + head.rot * (bill - b.head)).y);
            let leg = bird_at(Part::Leg(1), perched, 0.0, now);
            let foot = leg.pos + leg.rot * (b.foot - b.hip);
            assert!(foot.y.abs() < 0.005, "foot at {foot} at {now}");
        }
        assert!(
            (-0.004..0.012).contains(&lowest),
            "bill tip down to {lowest}"
        );

        let flying = AnimalState::Flying;
        assert!(!bird(Part::Beast(WING_FOLDED + 1), flying, 0.0).visible);
        assert!(!bird(Part::Leg(1), flying, 0.0).visible);
        assert!(bird(Part::Beast(TAIL_FAN), flying, 0.0).visible);
        // The wings beat: the hand is high on the upstroke and low on the downstroke.
        let up = bird(Part::Wing(3), flying, FRAC_PI_2);
        let down = bird(Part::Wing(3), flying, 3.0 * FRAC_PI_2);
        assert!(up.visible && down.visible);
        assert!(
            up.pos.y - down.pos.y > 0.06,
            "{} vs {}",
            up.pos.y,
            down.pos.y
        );
    }

    /// What the feet did on a run.
    #[derive(Debug)]
    struct Steps {
        /// How far the body went (m) and turned (rad).
        went: f32,
        turned: f32,
        /// How far the toes slid while on the ground (m, summed), and the most in one frame.
        slid: f32,
        worst: f32,
        /// The share of the time each foot was on the ground, and how often it lifted.
        down: [f32; 4],
        lifts: [u32; 4],
        /// The lowest a toe went (m).
        sink: f32,
    }

    /// Rabbit leg joints (shoulder or hip, wrist or heel, toe) of leg `n`.
    fn rabbit_leg(n: usize) -> [Vec3; 3] {
        let j = if n < 2 { RABBIT.front } else { RABBIT.hind };
        if n.is_multiple_of(2) { j.map(flip) } else { j }
    }

    /// The toe of every leg (owner frame).
    fn toes(species: Species, (male, young): (bool, bool), pose: &Pose) -> [Vec3; 4] {
        let scale = body_scale(species, male, young);
        std::array::from_fn(|k| {
            if species == Species::Rabbit {
                let j = rabbit_leg(k);
                let p = pose.place(Part::Beast(LEG_MID + k as u8));
                p.pos + p.rot * (j[2] - j[1])
            } else {
                let j = rig(species).leg(k as u8);
                let p = pose.place(Part::Beast(LEG_FOOT + k as u8));
                p.pos + p.rot * ((j[4] - j[3]) * scale)
            }
        })
    }

    /// Run an animal for `secs` s at 60 Hz, `course(t)` giving its velocity (in its own frame,
    /// x and z) and how fast it turns (rad/s), and watch its toes in the world.
    fn run(
        species: Species,
        (male, young): (bool, bool),
        state: AnimalState,
        crouch: bool,
        secs: f32,
        course: impl Fn(f32) -> (Vec2, f32),
    ) -> Steps {
        const GROUND: f32 = 0.006;
        let dt = 1.0 / 60.0;
        let mut m = Motion::default();
        let (mut pos, mut yaw) = (Vec3::ZERO, 0.0f32);
        let mut last: Option<[Vec3; 4]> = None;
        let mut out = Steps {
            went: 0.0,
            turned: 0.0,
            slid: 0.0,
            worst: 0.0,
            down: [0.0; 4],
            lifts: [0; 4],
            sink: f32::MAX,
        };
        let mut was_down = [true; 4];
        let n = (secs / dt).round() as usize;
        for f in 0..n {
            let t = f as f32 * dt;
            let (v, w) = course(t);
            yaw += w * dt;
            out.turned += (w * dt).abs();
            let facing = Quat::from_rotation_y(yaw);
            let vel = facing * Vec3::new(v.x, 0.0, v.y);
            if f > 0 {
                pos += vel * dt;
                out.went += vel.length() * dt;
            }
            let input = Input {
                species,
                male,
                young,
                state,
                since: 0.0,
                now: 1.0 + f64::from(t),
                phase: 0.0,
                at: pos.as_dvec3(),
                facing,
                vel,
                crouch,
                biting: false,
                vary: 0.37,
                senses: Senses::default(),
                bird: None,
            };
            let pose = animal(&input, &mut m);
            let pts = toes(species, (male, young), &pose).map(|p| pos + facing * p);
            for k in 0..4 {
                out.sink = out.sink.min(pts[k].y);
                let on = pts[k].y < GROUND;
                if on {
                    out.down[k] += 1.0 / n as f32;
                }
                if was_down[k] && !on {
                    out.lifts[k] += 1;
                }
                was_down[k] = on;
                if let Some(prev) = last
                    && on
                    && prev[k].y < GROUND
                {
                    let d = pts[k] - prev[k];
                    let d = Vec2::new(d.x, d.z).length();
                    out.slid += d;
                    out.worst = out.worst.max(d);
                }
            }
            last = Some(pts);
        }
        out
    }

    /// Speed (m/s) going from 0 to `top` from `t0` at `accel` m/s², and back down from `t1`.
    fn ramp(t: f32, t0: f32, t1: f32, top: f32, accel: f32) -> f32 {
        ((t - t0) * accel)
            .clamp(0.0, top)
            .min(((t1 - t) * accel).max(0.0))
    }

    #[test]
    fn rabbit_fore_paws_land_on_every_hop() {
        // Running flat out, at the game's rate and at a quarter of it slowed down, each fore
        // paw that comes down stays down: none lands past its reach and is snatched up again
        // in the same step, its footfall never seen.
        for hz in [60.0f32, 240.0] {
            for v in [3.0f32, 4.5, 6.0, 7.0, 8.5] {
                let dt = 1.0 / hz;
                let mut m = Motion::default();
                let (mut landed, mut lost) = (0, 0);
                let mut last = [f64::MIN; 2];
                for f in 0..(8.0 * hz) as usize {
                    let t = f as f32 * dt;
                    let now = 1.0 + f64::from(t);
                    let input = Input {
                        species: Species::Rabbit,
                        male: false,
                        young: false,
                        state: AnimalState::Flee,
                        since: 0.0,
                        now,
                        phase: 0.0,
                        at: DVec3::new(0.0, 0.0, -f64::from(v * t)),
                        facing: Quat::IDENTITY,
                        vel: Vec3::new(0.0, 0.0, -v),
                        crouch: false,
                        biting: false,
                        vary: 0.37,
                        senses: Senses::default(),
                        bird: None,
                    };
                    animal(&input, &mut m);
                    for (foot, last) in m.feet.iter().zip(&mut last) {
                        if f > 0 && foot.down == now && foot.down != *last {
                            landed += 1;
                            if foot.air.is_some() {
                                lost += 1;
                            }
                        }
                        *last = foot.down;
                    }
                }
                eprintln!("rabbit at {v} m/s, {hz} Hz: {landed} fore footfalls, {lost} lost");
                assert!(
                    landed > 20,
                    "rabbit at {v} m/s, {hz} Hz: only {landed} footfalls"
                );
                assert_eq!(
                    lost, 0,
                    "rabbit at {v} m/s, {hz} Hz: {lost} of {landed} fore footfalls lifted as they landed"
                );
            }
        }
    }

    #[test]
    fn feet_do_not_slide() {
        use AnimalState::{Charge, Flee, Graze, Stalk, Wander};
        type Course = fn(f32) -> (Vec2, f32);
        fn ahead(v: f32) -> Vec2 {
            Vec2::new(0.0, -v)
        }
        let cases: [(&str, Species, AnimalState, f32, Course); 40] = [
            ("deer walk", Species::Deer, Wander, 4.0, |_| {
                (ahead(1.3), 0.0)
            }),
            ("deer trot", Species::Deer, Wander, 3.0, |_| {
                (ahead(3.2), 0.0)
            }),
            ("deer canter", Species::Deer, Flee, 3.0, |_| {
                (ahead(6.5), 0.0)
            }),
            ("deer sprint", Species::Deer, Flee, 3.0, |_| {
                (ahead(11.5), 0.0)
            }),
            ("deer bolts", Species::Deer, Flee, 3.0, |t| {
                (ahead(ramp(t, 0.3, 99.0, 11.5, 16.0)), 0.0)
            }),
            ("deer start and stop", Species::Deer, Wander, 4.5, |t| {
                (ahead(ramp(t, 0.3, 3.2, 3.2, 6.0)), 0.0)
            }),
            ("deer gallop and stop", Species::Deer, Flee, 4.0, |t| {
                (ahead(ramp(t, 0.0, 2.2, 11.5, 16.0)), 0.0)
            }),
            ("deer walk round a bend", Species::Deer, Wander, 4.0, |_| {
                (ahead(1.3), 0.8)
            }),
            ("deer trot round a bend", Species::Deer, Wander, 3.0, |_| {
                (ahead(3.2), 1.0)
            }),
            ("deer turn on the spot", Species::Deer, Wander, 3.0, |t| {
                (Vec2::ZERO, if t < 1.5 { 1.5 } else { 0.0 })
            }),
            ("wolf walk", Species::Wolf, Wander, 4.0, |_| {
                (ahead(1.5), 0.0)
            }),
            ("wolf trot", Species::Wolf, Wander, 3.0, |_| {
                (ahead(3.5), 0.0)
            }),
            ("wolf lope", Species::Wolf, Wander, 3.0, |_| {
                (ahead(6.0), 0.0)
            }),
            ("wolf gallop", Species::Wolf, Flee, 3.0, |_| {
                (ahead(10.5), 0.0)
            }),
            ("wolf stalk", Species::Wolf, Stalk, 4.0, |_| {
                (ahead(1.0), 0.0)
            }),
            ("wolf start and stop", Species::Wolf, Wander, 4.5, |t| {
                (ahead(ramp(t, 0.3, 3.2, 7.0, 8.0)), 0.0)
            }),
            ("wolf trot round a bend", Species::Wolf, Wander, 3.0, |_| {
                (ahead(3.5), 1.2)
            }),
            ("wolf turn on the spot", Species::Wolf, Wander, 3.0, |t| {
                (Vec2::ZERO, if t < 1.5 { -2.0 } else { 0.0 })
            }),
            ("rabbit hop", Species::Rabbit, Wander, 4.0, |_| {
                (ahead(0.6), 0.0)
            }),
            ("rabbit run", Species::Rabbit, Wander, 3.0, |_| {
                (ahead(4.0), 0.0)
            }),
            ("rabbit sprint", Species::Rabbit, Flee, 3.0, |_| {
                (ahead(8.5), 0.0)
            }),
            ("rabbit start and stop", Species::Rabbit, Wander, 4.5, |t| {
                (ahead(ramp(t, 0.3, 3.2, 4.0, 8.0)), 0.0)
            }),
            (
                "rabbit hop round a bend",
                Species::Rabbit,
                Wander,
                4.0,
                |_| (ahead(0.6), 1.5),
            ),
            (
                "rabbit turn on the spot",
                Species::Rabbit,
                Wander,
                3.0,
                |t| (Vec2::ZERO, if t < 1.5 { 2.0 } else { 0.0 }),
            ),
            ("deer sidestep", Species::Deer, Wander, 3.0, |_| {
                (Vec2::new(0.8, -0.4), 0.0)
            }),
            // Stags are a size up.
            ("stag trot", Species::Deer, Wander, 3.0, |_| {
                (ahead(3.5), 0.0)
            }),
            ("stag gallop round a bend", Species::Deer, Flee, 3.0, |_| {
                (ahead(10.0), 0.5)
            }),
            ("wolf creep round", Species::Wolf, Stalk, 4.0, |_| {
                (ahead(1.0), -0.7)
            }),
            ("boar walk", Species::Boar, Wander, 4.0, |_| {
                (ahead(1.1), 0.0)
            }),
            ("boar trot", Species::Boar, Wander, 3.0, |_| {
                (ahead(3.0), 0.0)
            }),
            ("boar gallop", Species::Boar, Flee, 3.0, |_| {
                (ahead(9.0), 0.0)
            }),
            ("boar start and stop", Species::Boar, Wander, 4.5, |t| {
                (ahead(ramp(t, 0.3, 3.2, 3.0, 6.0)), 0.0)
            }),
            ("boar gallop and stop", Species::Boar, Flee, 4.0, |t| {
                (ahead(ramp(t, 0.0, 2.2, 9.0, 14.0)), 0.0)
            }),
            ("boar trot round a bend", Species::Boar, Wander, 3.0, |_| {
                (ahead(3.0), 1.2)
            }),
            ("boar turn on the spot", Species::Boar, Wander, 3.0, |t| {
                (Vec2::ZERO, if t < 1.5 { 1.8 } else { 0.0 })
            }),
            ("boar roots along", Species::Boar, Graze, 4.0, |_| {
                (ahead(0.3), 0.0)
            }),
            ("boar charges", Species::Boar, Charge, 3.0, |t| {
                (ahead(ramp(t, 0.2, 99.0, 9.5, 14.0)), 0.0)
            }),
            // Old boars are a size up, piglets a lot smaller.
            (
                "tusker gallop round a bend",
                Species::Boar,
                Flee,
                3.0,
                |_| (ahead(9.0), 0.6),
            ),
            ("piglet trot", Species::Boar, Wander, 3.0, |_| {
                (ahead(2.2), 0.0)
            }),
            ("piglet gallop", Species::Boar, Flee, 3.0, |_| {
                (ahead(7.0), 0.0)
            }),
        ];
        for (name, species, state, secs, course) in cases {
            let s = run(
                species,
                (
                    name.starts_with("stag") || name.starts_with("tusker"),
                    name.starts_with("piglet"),
                ),
                state,
                state == Stalk,
                secs,
                course,
            );
            eprintln!(
                "{name}: went {:.2} m, turned {:.2} rad, slid {:.4} m (worst {:.4} m in a frame), down {:.2?}, lifts {:?}, lowest toe {:.4} m",
                s.went, s.turned, s.slid, s.worst, s.down, s.lifts, s.sink
            );
            assert!(
                s.slid <= 0.01 * s.went + 0.01 && s.worst < 0.005,
                "{name}: the feet slid {:.3} m over {:.2} m (at most {:.4} m in a frame)",
                s.slid,
                s.went,
                s.worst
            );
            // A rabbit's paws stand still to within a hair, even the frame they peel off.
            if species == Species::Rabbit {
                assert!(
                    s.worst < 0.001,
                    "{name}: a paw slid {:.4} m in a frame",
                    s.worst
                );
            }
            if s.went > 1.0 {
                assert!(
                    s.lifts.iter().all(|&n| n >= 2),
                    "{name}: the feet hardly stepped ({:?})",
                    s.lifts
                );
            }
            if s.turned > 1.0 && s.went < 0.5 {
                assert!(
                    s.lifts.iter().all(|&n| n >= 1),
                    "{name}: the feet didn't step round ({:?})",
                    s.lifts
                );
            }
            assert!(
                s.down.iter().all(|&d| d > 0.05),
                "{name}: a foot was hardly down ({:?})",
                s.down
            );
            assert!(s.sink > -0.02, "{name}: a toe sank to {:.3} m", s.sink);
        }
    }

    /// One step of an animal in `state` (since then) at `pos` facing −Z at `speed` m/s.
    fn step(
        species: Species,
        (state, since): (AnimalState, f64),
        now: f64,
        (pos, speed): (Vec3, f32),
        senses: Senses,
        m: &mut Motion,
    ) -> Pose {
        let input = Input {
            species,
            male: false,
            young: false,
            state,
            since,
            now,
            phase: 0.0,
            at: pos.as_dvec3(),
            facing: Quat::IDENTITY,
            vel: Vec3::new(0.0, 0.0, -speed),
            crouch: false,
            biting: false,
            vary: 0.37,
            senses,
            bird: None,
        };
        animal(&input, m)
    }

    #[test]
    fn killed_animals_go_down_and_over_onto_their_side_and_lie_still() {
        for (species, speed) in [
            (Species::Deer, 10.0),
            (Species::Wolf, 10.0),
            (Species::Rabbit, 7.0),
        ] {
            // Running, then killed by a blow from its right at 1 s, the mover slowing it to
            // a stop; the body's height and roll (rad, left side down positive) from then on.
            let dt = 1.0 / 60.0;
            let mut m = Motion::default();
            let mut pos = Vec3::ZERO;
            let mut after = Vec::new();
            for f in 0..240 {
                let t = f as f32 * dt;
                let dead = f >= 60;
                let v = if dead {
                    (speed - 16.0 * (t - 1.0)).max(0.0)
                } else {
                    speed
                };
                pos.z -= v * dt;
                let state = if dead {
                    (AnimalState::Dead, 2.0)
                } else {
                    (AnimalState::Flee, 1.0)
                };
                let senses = Senses {
                    hurt: dead.then(|| (pos + Vec3::new(20.0, 0.0, 0.0)).as_dvec3()),
                    ..Senses::default()
                };
                let now = 1.0 + f64::from(t);
                let pose = step(species, state, now, (pos, v), senses, &mut m);
                if dead {
                    let body = pose.place(Part::Body);
                    let up = body.rot * Vec3::Y;
                    after.push((t - 1.0, body.pos.y, (-up.x).atan2(up.y)));
                }
            }
            let (h0, end) = (after[0].1, after[after.len() - 1]);
            eprintln!(
                "{species:?}: from {h0:.3} m to {:.3} m, rolled {:.2} rad",
                end.1, end.2
            );
            assert!(
                (end.2 - FRAC_PI_2).abs() < 0.15,
                "{species:?}: lies rolled {:.2} rad, not on its left side",
                end.2
            );
            assert!(end.1 < 0.6 * h0, "{species:?}: lies {:.3} m up", end.1);
            // It sinks as it goes over, not toppling stiff.
            let over = after
                .iter()
                .find(|a| a.2 > 0.5 * FRAC_PI_2)
                .expect("goes over");
            assert!(
                over.1 < h0 - 0.3 * (h0 - end.1),
                "{species:?}: half over at {:.2} s still {:.3} m up (from {h0:.3} m)",
                over.0,
                over.1
            );
            // And lies still.
            for a in after.iter().filter(|a| a.0 > 2.0) {
                assert!(
                    (a.1 - end.1).abs() < 0.002 && (a.2 - end.2).abs() < 0.01,
                    "{species:?}: still moving at {:.2} s",
                    a.0
                );
            }
        }
    }

    #[test]
    fn heads_turn_to_a_threat_before_the_neck() {
        for species in [Species::Deer, Species::Wolf] {
            // Standing, then a threat off to its left: the head (all it has to look with)
            // goes round first and the neck follows.
            let dt = 1.0 / 60.0;
            let mut m = Motion::default();
            let mut turn = Vec::new();
            for f in 0..240 {
                let t = f as f32 * dt;
                let now = 1.0 + f64::from(t);
                let alert = f >= 60;
                let state = if alert {
                    (AnimalState::Alert, 2.0)
                } else {
                    (AnimalState::Wander, 0.0)
                };
                let senses = Senses {
                    threat: alert.then_some(DVec3::new(-10.0, 0.0, -1.0)),
                    danger: if alert { 0.7 } else { 0.0 },
                    alarmed: if alert { 0.0 } else { 1e3 },
                    ..Senses::default()
                };
                let pose = step(species, state, now, (Vec3::ZERO, 0.0), senses, &mut m);
                let head = pose.place(Part::Head);
                let neck = pose.place(Part::Neck);
                let ahead = head.rot * Vec3::NEG_Z;
                let reach = head.pos - neck.pos;
                turn.push((
                    t - 1.0,
                    (-ahead.x).atan2(-ahead.z),
                    (-reach.x).atan2(-reach.z),
                ));
            }
            let at = |secs: f32| turn[60 + (secs / dt).round() as usize];
            let (soon, late) = (at(0.12), at(2.5));
            eprintln!(
                "{species:?}: head {:.2} neck {:.2} at 0.12 s; head {:.2} neck {:.2} at 2.5 s",
                soon.1, soon.2, late.1, late.2
            );
            assert!(
                soon.1 > 0.3,
                "{species:?}: head round only {:.2} rad",
                soon.1
            );
            assert!(
                soon.2 < 0.5 * soon.1,
                "{species:?}: neck round {:.2} rad with the head at {:.2}",
                soon.2,
                soon.1
            );
            assert!(
                late.2 > 0.3,
                "{species:?}: neck round only {:.2} rad",
                late.2
            );
        }
    }

    #[test]
    fn heads_turn_to_a_noise_before_the_neck() {
        for species in [Species::Deer, Species::Wolf, Species::Boar] {
            // Standing at ease, it hears one sound off to its left and senses nothing else:
            // the head goes round to it first and the neck follows, it keeps looking that
            // way a while, and then it looks away again.
            let dt = 1.0 / 60.0;
            let mut m = Motion::default();
            let mut turn = Vec::new();
            for f in 0..420 {
                let t = f as f32 * dt;
                let now = 1.0 + f64::from(t);
                let senses = Senses {
                    heard: (f == 60).then_some(DVec3::new(-10.0, 0.0, -1.0)),
                    alarmed: 1e3,
                    ..Senses::default()
                };
                let state = (AnimalState::Wander, 0.0);
                let pose = step(species, state, now, (Vec3::ZERO, 0.0), senses, &mut m);
                let head = pose.place(Part::Head);
                let neck = pose.place(Part::Neck);
                let ahead = head.rot * Vec3::NEG_Z;
                let reach = head.pos - neck.pos;
                turn.push((
                    t - 1.0,
                    (-ahead.x).atan2(-ahead.z),
                    (-reach.x).atan2(-reach.z),
                ));
            }
            let at = |secs: f32| turn[60 + (secs / dt).round() as usize];
            let (rest, soon, held, after) = (at(-0.1), at(0.12), at(2.3), at(5.9));
            eprintln!(
                "{species:?}: head {:.2} neck {:.2} at 0.12 s; head {:.2} neck {:.2} at 2.3 s; head {:.2} at 5.9 s",
                soon.1, soon.2, held.1, held.2, after.1
            );
            assert!(
                soon.1 - rest.1 > 0.3,
                "{species:?}: head round only {:.2} rad",
                soon.1 - rest.1
            );
            assert!(
                soon.2 - rest.2 < 0.5 * (soon.1 - rest.1),
                "{species:?}: neck round {:.2} rad with the head at {:.2}",
                soon.2 - rest.2,
                soon.1 - rest.1
            );
            assert!(
                held.2 - rest.2 > 0.3,
                "{species:?}: neck round only {:.2} rad",
                held.2 - rest.2
            );
            assert!(
                (after.1 - rest.1).abs() < 0.3 * (held.1 - rest.1),
                "{species:?}: still looking {:.2} rad round at 5.9 s",
                after.1 - rest.1
            );
        }
    }

    #[test]
    fn a_flinch_does_not_jump_when_the_state_changes() {
        for species in [Species::Deer, Species::Wolf, Species::Boar] {
            // Grazing, then struck from its right at 1 s; it bolts on its next senses tick,
            // the flinch still going: the head, carried by the neck, moves on smoothly.
            let dt = 1.0 / 60.0;
            let mut m = Motion::default();
            let mut heads = Vec::new();
            for f in 0..120 {
                let t = f as f32 * dt;
                let now = 1.0 + f64::from(t);
                let hit = f >= 60;
                let state = if f >= 72 {
                    (AnimalState::Flee, 1.0 + 12.0 * f64::from(dt))
                } else {
                    (AnimalState::Wander, 0.0)
                };
                let senses = Senses {
                    wound: hit.then(|| ((t - 1.0).max(0.0), DVec3::new(10.0, 0.0, 0.0))),
                    ..Senses::default()
                };
                let pose = step(species, state, now, (Vec3::ZERO, 0.0), senses, &mut m);
                let body = pose.place(Part::Body);
                let head = pose.place(Part::Head);
                heads.push(body.rot.inverse() * head.rot);
            }
            let jump = |f: usize| heads[f].angle_between(heads[f - 1]);
            let worst = (66..80).map(jump).fold(0.0f32, f32::max);
            let change = jump(72);
            eprintln!("{species:?}: head turns {change:.3} rad as it bolts, at most {worst:.3}");
            let before = (62..72).map(jump).fold(0.0f32, f32::max);
            assert!(
                change < 0.05 && change <= before + 0.01,
                "{species:?}: the head jumps {change:.3} rad as the state changes (at most {before:.3} before)"
            );
        }
    }

    #[test]
    fn a_tuskers_upper_tusks_stay_in_its_lip_as_it_slashes() {
        // Charging and slashing, its mouth opening and shutting: the lower tusks go with the
        // jaw, the upper ones stay with the head.
        let dt = 1.0 / 60.0;
        let mut m = Motion::default();
        let mut gaped = 0.0f32;
        for f in 0..120 {
            let t = f as f32 * dt;
            let input = Input {
                species: Species::Boar,
                male: true,
                young: false,
                state: AnimalState::Charge,
                since: 1.0,
                now: 1.0 + f64::from(t),
                phase: 0.0,
                at: DVec3::new(0.0, 0.0, -f64::from(5.0 * t)),
                facing: Quat::IDENTITY,
                vel: Vec3::new(0.0, 0.0, -5.0),
                crouch: false,
                biting: f > 60,
                vary: 0.37,
                senses: Senses::default(),
                bird: None,
            };
            let pose = animal(&input, &mut m);
            let head = pose.place(Part::Head);
            let jaw = pose.place(Part::Snout);
            let upper = pose.place(Part::Beast(UPPER_TUSKS));
            let lower = pose.place(Part::Beast(TUSKS));
            assert!(upper.visible && lower.visible);
            assert!(
                upper.pos.distance(head.pos) < 1e-5 && upper.rot.angle_between(head.rot) < 2e-3,
                "upper tusks off the head at {t:.2} s"
            );
            assert!(
                lower.pos.distance(jaw.pos) < 1e-5 && lower.rot.angle_between(jaw.rot) < 2e-3,
                "lower tusks off the jaw at {t:.2} s"
            );
            gaped = gaped.max(jaw.rot.angle_between(head.rot));
        }
        assert!(gaped > 0.1, "the mouth hardly opened ({gaped:.3} rad)");
    }

    #[test]
    fn rooting_boars_put_their_snouts_in_the_soil() {
        // Over a few shoves of the snout, its disc goes into the soil and comes up out of it,
        // staying down at the ground, while the feet stay planted.
        let (mut low, mut high) = (f32::MAX, f32::MIN);
        for i in 0..160 {
            let now = 100.0 + i as f64 * 0.025;
            let p = pose(Species::Boar, Part::Head, AnimalState::Graze, 0.0, 0.0, now);
            let disc = p.pos + p.rot * (art::BOAR_HEAD_FWD * 0.43 - art::BOAR_HEAD_UP * 0.014);
            low = low.min(disc.y);
            high = high.max(disc.y);
            assert!(disc.z < -0.6, "rooting snout in front of the chest: {disc}");
        }
        eprintln!("rooting snout from {low:.3} m to {high:.3} m");
        assert!((-0.03..0.02).contains(&low), "snout down to {low}");
        assert!(high > low + 0.02 && high < 0.12, "snout up to {high}");
        for k in 0..4 {
            let t = toe(Species::Boar, k, AnimalState::Graze, 0.0, 0.0, 100.0);
            assert!(t.y.abs() < 0.01, "rooting boar leg {k}: toe at {t}");
        }
    }

    #[test]
    fn a_hurt_animal_hitches_in_its_stride() {
        for species in [Species::Deer, Species::Wolf, Species::Boar] {
            // Trotting, one struck at 1 s and one not: the struck one's rhythm checks for a
            // beat (its feet fall behind the other's) and then runs on at the same rate.
            let dt = 1.0 / 60.0;
            let (mut hurt, mut whole) = (Motion::default(), Motion::default());
            let speed = 3.0 * body_scale(species, false, false);
            let mut lag = Vec::new();
            for f in 0..150 {
                let t = f as f32 * dt;
                let now = 1.0 + f64::from(t);
                let pos = Vec3::new(0.0, 0.0, -speed * t);
                let senses = Senses {
                    wound: (f >= 60).then(|| ((t - 1.0).max(0.0), DVec3::new(10.0, 0.0, 0.0))),
                    ..Senses::default()
                };
                let state = (AnimalState::Wander, 0.0);
                step(species, state, now, (pos, speed), senses, &mut hurt);
                step(
                    species,
                    state,
                    now,
                    (pos, speed),
                    Senses::default(),
                    &mut whole,
                );
                lag.push((t - 1.0, (whole.cycles - hurt.cycles) as f32));
            }
            let at = |secs: f32| lag[60 + (secs / dt).round() as usize].1;
            eprintln!(
                "{species:?}: {:.3} cycles behind at 0.7 s, {:.3} at 1.4 s",
                at(0.7),
                at(1.4)
            );
            assert!(
                at(-0.05).abs() < 1e-4,
                "{species:?}: out of step before the blow"
            );
            assert!(
                at(0.7) > 0.15,
                "{species:?}: hitched only {:.3} cycles",
                at(0.7)
            );
            assert!(
                (at(1.4) - at(0.7)).abs() < 0.03,
                "{species:?}: still hitching after 0.7 s"
            );
        }
    }

    #[test]
    fn hurt_animals_flinch_away_from_the_blow() {
        for species in [Species::Deer, Species::Wolf, Species::Boar] {
            // Standing, then struck from its right at 1 s and not killed: the body jolts away
            // from the blow, rolling off it, the ears go flat back, a foot steps to catch it,
            // and it gathers itself.
            let dt = 1.0 / 60.0;
            let mut m = Motion::default();
            let mut after = Vec::new();
            let mut rest = None;
            for f in 0..180 {
                let t = f as f32 * dt;
                let now = 1.0 + f64::from(t);
                let hit = f >= 60;
                let senses = Senses {
                    wound: hit.then(|| ((t - 1.0).max(0.0), DVec3::new(10.0, 0.0, 0.0))),
                    ..Senses::default()
                };
                let pose = step(
                    species,
                    (AnimalState::Wander, 0.0),
                    now,
                    (Vec3::ZERO, 0.0),
                    senses,
                    &mut m,
                );
                let body = pose.place(Part::Body);
                let up = body.rot * Vec3::Y;
                let ear = pose.place(Part::Ear(1)).rot;
                let ear = pose.place(Part::Head).rot.inverse() * ear;
                let lift = toes(species, (false, false), &pose)
                    .iter()
                    .map(|p| p.y)
                    .fold(0.0, f32::max);
                let fwd = body.rot * Vec3::NEG_Z;
                let ahead = pose.place(Part::Head).rot * Vec3::NEG_Z;
                let turn = (-ahead.x).atan2(-ahead.z);
                let now = (body.pos, (-up.x).atan2(up.y), ear, lift, -fwd.y, turn);
                if hit {
                    after.push((t - 1.0, now));
                } else {
                    rest = Some(now);
                }
            }
            let rest = rest.expect("stood");
            let (mut away, mut roll, mut pinned, mut stepped) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
            let (mut dip, mut sank, mut swung) = (0.0f32, 0.0f32, 0.0f32);
            for (t, (pos, r, ear, lift, nose, turn)) in &after {
                if (0.1..0.4).contains(t) {
                    swung = swung.max(turn - rest.5);
                }
                if *t < 0.3 {
                    away = away.max(rest.0.x - pos.x);
                    roll = roll.max(*r - rest.1);
                }
                if (0.15..0.45).contains(t) {
                    dip = dip.max(nose - rest.4);
                }
                if *t < 0.45 {
                    sank = sank.max(rest.0.y - pos.y);
                }
                if (0.2..0.8).contains(t) {
                    pinned = pinned.max(ear.angle_between(rest.2));
                }
                if *t < 0.9 {
                    stepped = stepped.max(*lift);
                }
            }
            eprintln!(
                "{species:?}: jolted {away:.3} m away, rolled {roll:.2} rad, ears turned {pinned:.2} rad, a foot up {stepped:.3} m, stumbled {dip:.3} rad nose down, sank {sank:.3} m, head swung {swung:.2} rad away"
            );
            assert!(away > 0.05, "{species:?}: jolted only {away:.3} m");
            assert!(roll > 0.1, "{species:?}: rolled only {roll:.2} rad");
            assert!(
                pinned > 0.8,
                "{species:?}: ears turned only {pinned:.2} rad"
            );
            assert!(stepped > 0.02, "{species:?}: no foot stepped");
            assert!(dip > 0.15, "{species:?}: stumbled only {dip:.3} rad");
            assert!(sank > 0.06, "{species:?}: its legs gave only {sank:.3} m");
            assert!(
                swung > 0.8,
                "{species:?}: the head swung only {swung:.2} rad away"
            );
            for (t, (pos, r, _, _, _, _)) in after.iter().filter(|a| a.0 > 1.3) {
                assert!(
                    (pos.x - rest.0.x).abs() < 0.01 && (r - rest.1).abs() < 0.02,
                    "{species:?}: still flinching at {t:.2} s"
                );
            }
        }
    }
}
