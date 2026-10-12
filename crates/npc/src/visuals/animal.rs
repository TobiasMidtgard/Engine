//! Animal bodies: red deer, wolves, rabbits and starlings built from the sculpted pieces in
//! [`crate::art::animals`] (cheap primitives without the renderer) and posed every step from
//! what the animal is doing.
//!
//! Deer and wolves plant their feet: each leg follows a footfall pattern (walk, trot, gallop,
//! blended by speed) and is solved with inverse kinematics from the shoulder or hip to the
//! toe, so stance feet stay on the ground while the body moves over them and swinging legs
//! fold at the carpus or hock. Heads, ears and tails take a look per state (grazing head-down,
//! alert head-up with ears forward, a flagging tail when fleeing, a wolf's low stalk and bite),
//! animals lie down to rest with folded legs and fall onto their side when killed. Rabbits hop
//! and sit up; starlings hop, peck, flap with folding hands, spread their tail and bank.

use std::f32::consts::FRAC_PI_2;

use engine_core::prelude::*;
use engine_render::{MeshRenderer, Shape, StandardMaterial};
use engine_world::locomotion::{LocomotionMode, LocomotionState};

use super::*;
use crate::art::ArtAssets;
use crate::art::animals::{self as art, BIRD, DEER, Piece, QuadRig, RABBIT, STAG_SCALE, WOLF};
use crate::wildlife::{Animal, AnimalState, Species};

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

fn part_of(piece: Piece) -> Part {
    match piece {
        Piece::Body => Part::Body,
        Piece::Neck => Part::Neck,
        Piece::Neck2 => Part::Beast(NECK2),
        Piece::Head => Part::Head,
        Piece::Gloss => Part::Beast(GLOSS),
        Piece::Jaw => Part::Snout,
        Piece::Ear(s) => Part::Ear(s),
        Piece::Tail => Part::Tail,
        Piece::Tail2 => Part::Beast(TAIL2),
        Piece::Antlers => Part::Antler(0),
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

fn piece_of(part: Part) -> Option<Piece> {
    Some(match part {
        Part::Body => Piece::Body,
        Part::Neck => Piece::Neck,
        Part::Head => Piece::Head,
        Part::Snout => Piece::Jaw,
        Part::Ear(s) => Piece::Ear(s),
        Part::Tail => Piece::Tail,
        Part::Antler(_) => Piece::Antlers,
        Part::Leg(k) => Piece::Leg(k, 0),
        Part::Wing(w) => Piece::Wing(w % 2, w / 2),
        Part::Beast(b) => match b {
            0..4 => Piece::Leg(b - LEG_MID, 1),
            4..8 => Piece::Leg(b - LEG_LOW, 2),
            8..12 => Piece::Leg(b - LEG_FOOT, 3),
            12..16 => Piece::Hoof(b - HOOF),
            GLOSS => Piece::Gloss,
            NECK2 => Piece::Neck2,
            TAIL2 => Piece::Tail2,
            TAIL_FAN => Piece::TailFan,
            20..22 => Piece::Wing(b - WING_FOLDED, 2),
            _ => return None,
        },
        _ => return None,
    })
}

fn body_scale(species: Species, male: bool) -> f32 {
    if species == Species::Deer && male {
        STAG_SCALE
    } else {
        1.0
    }
}

/// The parts of an animal's body.
pub(super) fn parts(
    stand_in: &StandIn,
    animal: Option<&Animal>,
    species: Species,
    art: &mut ArtAssets,
) -> Vec<(Part, MeshRenderer, Vec3)> {
    if !art::sculpted(species) {
        return stand_in::parts(stand_in, animal, species);
    }
    let male = animal.is_some_and(|a| a.male);
    let scale = Vec3::splat(body_scale(species, male));
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
    if let Some(pieces) = art::pieces(species, male, tint, art) {
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
        },
        segments: 6,
    };
    art::piece_names(species, male)
        .into_iter()
        .map(|piece| (part_of(piece), MeshRenderer::new(blob, color), scale))
        .collect()
}

/// What the pose of one animal depends on.
struct Ctx<'a> {
    state: AnimalState,
    /// Seconds in the current state.
    t: f32,
    now: f32,
    /// Per-animal variety 0..1.
    seed: f32,
    phase: f32,
    speed: f32,
    loco: &'a LocomotionState,
    biting: bool,
    flap: f32,
}

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
    if !art::sculpted(species) {
        return stand_in::pose(
            species, part, state, since, now, phase, speed, loco, biting, flap, male,
        );
    }
    sculpted_pose(
        species, part, state, since, now, phase, speed, loco, biting, flap, male,
    )
}

/// The pose of one piece of a sculpted body.
#[allow(clippy::too_many_arguments)]
fn sculpted_pose(
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
    let Some(piece) = piece_of(part) else {
        return HIDDEN;
    };
    let c = Ctx {
        state,
        t: (now - since).max(0.0) as f32,
        now: (now % 10_000.0) as f32,
        seed: ((since * 0.618_034).fract() as f32 + 1.0).fract(),
        phase,
        speed,
        loco,
        biting,
        flap,
    };
    let p = match species {
        Species::Deer => Quad::new(&DEER, true, &c).place(piece, male),
        Species::Wolf => Quad::new(&WOLF, false, &c).place(piece, false),
        Species::Rabbit => rabbit(piece, &c),
        Species::Bird => bird(piece, &c),
    };
    let s = body_scale(species, male);
    Placed {
        pos: p.pos * s,
        ..p
    }
}

// ======================================================================= shared helpers

fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
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

// ======================================================================= deer and wolves

/// Footfall pattern: when each leg (left front, right front, left hind, right hind) touches
/// down in the cycle, the share of the cycle a foot is on the ground and how high it lifts.
#[derive(Debug, Clone, Copy)]
struct Gait {
    offsets: [f32; 4],
    duty: f32,
    lift: f32,
    /// Carpus/hock flex in the swing, radians.
    flex: f32,
}

const WALK: Gait = Gait {
    offsets: [0.25, 0.75, 0.0, 0.5],
    duty: 0.6,
    lift: 0.07,
    flex: 0.9,
};
const TROT: Gait = Gait {
    offsets: [0.0, 0.5, 0.5, 0.0],
    duty: 0.42,
    lift: 0.12,
    flex: 1.5,
};
/// Transverse gallop (deer).
const GALLOP: Gait = Gait {
    offsets: [0.42, 0.52, 0.0, 0.08],
    duty: 0.28,
    lift: 0.16,
    flex: 1.7,
};
/// Rotary gallop (wolves).
const ROTARY: Gait = Gait {
    offsets: [0.55, 0.45, 0.0, 0.1],
    duty: 0.3,
    lift: 0.12,
    flex: 1.6,
};

/// Where the head, ears and tail are, relative to the rest pose (radians).
#[derive(Debug, Clone, Copy, Default)]
struct Look {
    neck: f32,
    neck2: f32,
    head: f32,
    yaw: f32,
    /// Ears turned forward (+) or back (−) and raised (+) or drooping (−).
    ear_fwd: f32,
    ear_up: f32,
    /// Tail raised (+).
    tail: f32,
    tail2: f32,
    tail_swish: f32,
    jaw: f32,
    /// Forequarters lowered (body pitched nose-down), radians.
    stoop: f32,
}

impl Look {
    fn mix(self, o: Look, t: f32) -> Look {
        let l = |a: f32, b: f32| a + (b - a) * t;
        Look {
            neck: l(self.neck, o.neck),
            neck2: l(self.neck2, o.neck2),
            head: l(self.head, o.head),
            yaw: l(self.yaw, o.yaw),
            ear_fwd: l(self.ear_fwd, o.ear_fwd),
            ear_up: l(self.ear_up, o.ear_up),
            tail: l(self.tail, o.tail),
            tail2: l(self.tail2, o.tail2),
            tail_swish: l(self.tail_swish, o.tail_swish),
            jaw: l(self.jaw, o.jaw),
            stoop: l(self.stoop, o.stoop),
        }
    }
}

/// A posed deer or wolf.
struct Quad<'a> {
    rig: &'a QuadRig,
    deer: bool,
    c: &'a Ctx<'a>,
    body: Frame,
    look: Look,
    /// Gaits in use with their cycle position, stance stride (rig meters) and weight.
    gaits: [(Gait, f32, f32, f32); 2],
    /// How much the legs step (0 standing).
    stepping: f32,
    /// Lying down 0..1 and dead 0..1.
    lie: f32,
    dead: f32,
}

impl<'a> Quad<'a> {
    fn new(rig: &'a QuadRig, deer: bool, c: &'a Ctx<'a>) -> Self {
        let st = c.state;
        let speed = c.speed;
        let dead = if st == AnimalState::Dead {
            (c.t / 0.75).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let lie = if st == AnimalState::Rest {
            smooth(0.0, 1.8, c.t)
        } else {
            0.0
        };
        let crouch = st == AnimalState::Stalk || c.loco.mode == LocomotionMode::Crouch;
        // Gaits by speed: walk, trot, gallop (time-based so it keeps a natural beat).
        let (trot_at, gallop_at) = if deer { (1.9, 5.2) } else { (2.0, 6.0) };
        let w_trot = smooth(trot_at - 0.4, trot_at + 0.4, speed);
        let w_gallop = smooth(gallop_at - 0.7, gallop_at + 0.6, speed);
        let cycle_d = 2.0 * if deer { 0.9 } else { 0.8 };
        let t_d = c.phase / TAU;
        let freq = if deer { 2.2 } else { 2.5 };
        let t_g = (c.now * freq).fract();
        let gallop_stride = (speed / freq).min(if deer { 4.4 } else { 3.6 });
        let fast = if deer { GALLOP } else { ROTARY };
        let gaits = if w_gallop > 0.0 {
            [
                (TROT, t_d, TROT.duty * cycle_d, 1.0 - w_gallop),
                (fast, t_g, fast.duty * gallop_stride, w_gallop),
            ]
        } else {
            [
                (WALK, t_d, WALK.duty * cycle_d, 1.0 - w_trot),
                (TROT, t_d, TROT.duty * cycle_d, w_trot),
            ]
        };
        let stepping = smooth(0.08, 0.45, speed) * (1.0 - lie) * (1.0 - dead);
        // Body bob and pitch from the gait.
        let mut bob = 0.0;
        let mut pitch = 0.0;
        for (g, t, _, w) in gaits {
            let (b, p) = if g.duty < 0.35 {
                (
                    0.045 * (TAU * (t - 0.7)).sin(),
                    0.09 * (TAU * (t - 0.12)).cos(),
                )
            } else if g.duty < 0.5 {
                (-0.022 * (2.0 * TAU * (t - 0.2)).cos(), 0.0)
            } else {
                (-0.01 * (2.0 * TAU * t).cos(), 0.012 * (TAU * t).sin())
            };
            bob += b * w;
            pitch += p * w;
        }
        bob *= stepping;
        pitch *= stepping;
        let look = Self::look(deer, c, crouch, w_gallop);
        pitch -= look.stoop;
        let mut pos = rig.body + Vec3::Y * bob;
        if crouch {
            pos.y -= if deer { 0.12 } else { 0.13 };
            pitch -= 0.04;
        }
        // Breathing.
        pos.y += 0.003 * (c.now * 1.6 + c.seed * 9.0).sin();
        let mut rot = Quat::from_rotation_x(pitch);
        if lie > 0.0 {
            pos.y -= (rig.belly - if deer { 0.1 } else { 0.06 }) * lie;
            rot = Quat::from_rotation_x(pitch * (1.0 - lie) + 0.03 * lie);
        }
        if dead > 0.0 {
            let side = if c.seed > 0.5 { 1.0 } else { -1.0 };
            let f = dead * dead;
            pos.y = pos.y + (rig.half_width + 0.02 - pos.y) * f;
            rot = Quat::from_rotation_z(side * FRAC_PI_2 * f) * rot;
        }
        let body = Frame {
            origin: rig.body,
            pos,
            rot,
        };
        Quad {
            rig,
            deer,
            c,
            body,
            look,
            gaits,
            stepping,
            lie,
            dead,
        }
    }

    fn look(deer: bool, c: &Ctx, crouch: bool, gallop: f32) -> Look {
        let st = c.state;
        let moving = smooth(0.25, 0.8, c.speed);
        let ease = |secs: f32| smooth(0.0, secs, c.t);
        let calm = Look {
            yaw: 0.25 * drift(c.now, 0.23, c.seed),
            ear_fwd: 0.25 * drift(c.now, 0.4, c.seed + 0.3),
            tail_swish: 0.25 * twitch(c.now, 3.1, c.seed) * (c.now * 14.0).sin(),
            ..Look::default()
        };
        let mut l = calm;
        // Walking and trotting: head a little lower, nodding with the stride.
        let nod = (2.0 * c.phase).sin() * 0.04 * moving;
        let walk = Look {
            neck: if deer { -0.12 } else { -0.15 } + nod,
            head: 0.08 - nod,
            yaw: 0.0,
            ear_fwd: 0.1,
            ..calm
        };
        l = l.mix(walk, moving);
        let target = match st {
            AnimalState::Graze | AnimalState::Drink | AnimalState::Feed => {
                let chew = (c.now * if deer { 5.0 } else { 7.0 }).sin();
                let down = Look {
                    neck: if deer { -1.65 } else { -1.9 },
                    neck2: if deer { -0.3 } else { 0.0 },
                    head: if deer { 1.22 } else { 0.9 } + 0.03 * chew,
                    stoop: if deer { 0.05 } else { 0.12 },
                    yaw: 0.12 * drift(c.now, 0.5, c.seed),
                    ear_fwd: 0.4 * twitch(c.now, 2.3, c.seed) - 0.1,
                    ear_up: -0.15,
                    jaw: if st == AnimalState::Feed {
                        -0.25 - 0.2 * chew
                    } else {
                        0.0
                    },
                    tail_swish: calm.tail_swish,
                    ..Look::default()
                };
                // Graze while standing or stepping slowly; head up to walk.
                Some((down, 0.7 * (1.0 - smooth(0.5, 1.0, c.speed))))
            }
            AnimalState::Alert => Some((
                Look {
                    neck: 0.3,
                    neck2: if deer { 0.18 } else { 0.0 },
                    head: if deer { 0.5 } else { 0.15 },
                    yaw: 0.35 * drift(c.now, 0.6, c.seed),
                    ear_fwd: 0.7,
                    ear_up: 0.35,
                    tail: if deer { 0.5 } else { 0.7 },
                    ..Look::default()
                },
                0.35,
            )),
            AnimalState::Flee | AnimalState::Chase => Some((
                Look {
                    neck: -0.35,
                    neck2: if deer { -0.05 } else { 0.0 },
                    head: if deer { 0.45 } else { 0.4 },
                    ear_fwd: if deer { -0.6 } else { -0.8 },
                    ear_up: -0.1,
                    tail: if deer {
                        1.9
                    } else if st == AnimalState::Chase {
                        0.95
                    } else {
                        -0.25
                    },
                    tail2: if deer { 0.0 } else { -0.3 },
                    ..Look::default()
                },
                0.3 + 0.5 * (1.0 - gallop),
            )),
            AnimalState::Stalk => Some((
                Look {
                    neck: -0.4,
                    head: 0.35,
                    ear_fwd: 0.6,
                    ear_up: 0.2,
                    tail: -0.15,
                    tail2: 0.1,
                    ..Look::default()
                },
                0.5,
            )),
            AnimalState::Rest => Some((
                Look {
                    neck: if deer { 0.1 } else { -0.1 },
                    neck2: if deer { 0.12 } else { 0.0 },
                    head: if deer { 0.2 } else { 0.25 },
                    yaw: 0.4 * drift(c.now, 0.15, c.seed),
                    ear_fwd: 0.2 * drift(c.now, 0.3, c.seed),
                    ..Look::default()
                },
                1.2,
            )),
            AnimalState::Dead => Some((
                Look {
                    neck: -0.25,
                    neck2: -0.35,
                    head: -0.2,
                    yaw: 0.9,
                    ear_fwd: -0.3,
                    ear_up: -0.7,
                    tail: -0.2,
                    ..Look::default()
                },
                0.7,
            )),
            _ => None,
        };
        if let Some((t, secs)) = target {
            l = l.mix(t, ease(secs));
        }
        if crouch && st != AnimalState::Stalk {
            l.neck -= 0.25;
            l.head += 0.2;
        }
        if c.biting {
            let snap = (c.now * 14.0).sin().abs();
            l.neck -= 0.15;
            l.head += 0.3;
            l.jaw = -0.75 * snap;
            l.ear_fwd = -0.6;
        }
        l
    }

    fn place(&self, piece: Piece, _male: bool) -> Placed {
        match piece {
            Piece::Body => self.body.placed(),
            Piece::Leg(k, s) => self.leg(k, s.min(3)),
            Piece::Hoof(k) => self.leg(k, 3),
            Piece::Neck => self.neck(0).placed(),
            Piece::Neck2 => self.neck(1).placed(),
            Piece::Head | Piece::Gloss | Piece::Antlers => self.head().placed(),
            Piece::Jaw => self
                .head()
                .child(self.rig.jaw, Quat::from_rotation_x(self.look.jaw))
                .placed(),
            Piece::Ear(s) => self.ear(s).placed(),
            Piece::Tail => self.tail(0).placed(),
            Piece::Tail2 => self.tail(1).placed(),
            _ => HIDDEN,
        }
    }

    fn neck(&self, seg: u8) -> Frame {
        let l = &self.look;
        let n1 = self.body.child(
            self.rig.neck,
            Quat::from_rotation_y(l.yaw * 0.5) * Quat::from_rotation_x(l.neck),
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
            Quat::from_rotation_y(l.yaw * 0.25)
                * Quat::from_rotation_x(l.head)
                * Quat::from_rotation_z(roll),
        )
    }

    fn ear(&self, s: u8) -> Frame {
        let l = &self.look;
        let (up, fwd) = if self.deer {
            (art::DEER_HEAD_UP, art::DEER_HEAD_FWD)
        } else {
            (Vec3::Y, Vec3::NEG_Z)
        };
        let side = if s == 0 { -1.0 } else { 1.0 };
        let flick = 0.5 * twitch(self.c.now, 2.7, self.c.seed + s as f32 * 0.37);
        let swivel = l.ear_fwd + flick * if s == 0 { 1.0 } else { -0.6 };
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

    fn tail(&self, seg: u8) -> Frame {
        let l = &self.look;
        let t1 = self.body.child(
            self.rig.tail,
            Quat::from_rotation_y(l.tail_swish) * Quat::from_rotation_x(-l.tail),
        );
        if seg == 0 {
            return t1;
        }
        let sway = if self.deer {
            0.0
        } else {
            0.12 * (self.c.phase * 2.0).sin() * self.stepping
        };
        t1.child(
            self.rig.tail2,
            Quat::from_rotation_y(sway + l.tail_swish) * Quat::from_rotation_x(-l.tail2),
        )
    }

    /// Where a stance or swinging foot is (owner space, rig units), how far the foot turns
    /// from its rest angle and how much the carpus or hock folds.
    fn foot_target(&self, k: u8, toe: Vec3) -> (Vec3, f32, f32) {
        let front = k < 2;
        let mut off = Vec3::ZERO;
        let mut foot = 0.0;
        let mut flex = 0.0;
        for (g, t, stride, w) in self.gaits {
            if w <= 0.0 {
                continue;
            }
            let lt = (t - g.offsets[k as usize]).rem_euclid(1.0);
            let (dz, dy, fd, fl) = if lt < g.duty {
                let s = lt / g.duty;
                (stride * (s - 0.5), 0.0, -0.45 * smooth(0.65, 1.0, s), 0.0)
            } else {
                let s = (lt - g.duty) / (1.0 - g.duty);
                let e = s * s * (3.0 - 2.0 * s);
                let up = (PI * s).sin().max(0.0).powf(0.8) * (1.15 - 0.3 * s);
                let fold = (PI * s.powf(0.75)).sin().max(0.0);
                (
                    stride * (0.5 - e),
                    g.lift * up,
                    -(if front { 1.3 } else { 0.9 }) * fold,
                    g.flex * fold * if front { -1.0 } else { 0.75 },
                )
            };
            off += Vec3::new(0.0, dy, dz) * w;
            foot += fd * w;
            flex += fl * w;
        }
        let a = self.stepping;
        (Vec3::new(toe.x, 0.0, toe.z) + off * a, foot * a, flex * a)
    }

    fn leg(&self, k: u8, seg: u8) -> Placed {
        let rig = self.rig;
        let j = rig.leg(k);
        let front = k < 2;
        let mut rest = [0.0; 4];
        let mut len = [0.0; 4];
        for i in 0..4 {
            let d = yz(j[i + 1]) - yz(j[i]);
            rest[i] = angle(d);
            len[i] = d.length();
        }
        let mut ang = rest;
        let stand = 1.0 - self.lie.max(self.dead);
        if stand > 0.0 {
            // Inverse kinematics from the shoulder or hip to the toe.
            let (toe_owner, foot_d, flex) = self.foot_target(k, j[4]);
            let toe = rig.body + self.body.rot.inverse() * (toe_owner - self.body.pos);
            let a3 = rest[3] + foot_d;
            let f3 = yz(toe) - dir(a3) * len[3];
            let ds = wrap_pi(angle(f3 - yz(j[0])) - angle(yz(j[3]) - yz(j[0])));
            let a2 = rest[2] + ds + flex;
            let j2 = f3 - dir(a2) * len[2];
            let d = j2 - yz(j[0]);
            let dist = d
                .length()
                .clamp((len[0] - len[1]).abs() + 1e-3, len[0] + len[1] - 1e-3);
            let cos_a = ((len[0] * len[0] + dist * dist - len[1] * len[1]) / (2.0 * len[0] * dist))
                .clamp(-1.0, 1.0);
            let al = cos_a.acos();
            let a0 = angle(d) + if front { -al } else { al };
            let j1 = yz(j[0]) + dir(a0) * len[0];
            let a1 = angle(j2 - j1);
            ang = [a0, a1, a2, a3];
        }
        let fold = self.fold(front);
        let w_fold = smooth(0.0, 0.7, self.lie);
        for i in 0..4 {
            ang[i] += wrap_pi(fold[i] - ang[i]) * w_fold;
        }
        if self.dead > 0.0 {
            let limp = [
                rest[0] + 0.15,
                rest[1] - 0.15,
                rest[2] + 0.25,
                rest[3] - 0.2,
            ];
            let limp = if front {
                limp
            } else {
                [rest[0] - 0.2, rest[1] + 0.25, rest[2] - 0.15, rest[3] - 0.3]
            };
            for i in 0..4 {
                ang[i] += wrap_pi(limp[i] - ang[i]) * self.dead;
            }
        }
        // Forward kinematics from the attachment.
        let mut p = j[0];
        for i in 0..seg as usize {
            let d = dir(ang[i]) * len[i];
            p += Vec3::new(j[i + 1].x - j[i].x, d.x, d.y);
        }
        let side = if k.is_multiple_of(2) { -1.0 } else { 1.0 };
        let splay = if front { 0.08 } else { 0.32 } * w_fold + 0.12 * self.dead;
        let rab = Quat::from_rotation_z(side * splay);
        let pos = j[0] + rab * (p - j[0]);
        let rot = rab * Quat::from_rotation_x(ang[seg as usize] - rest[seg as usize]);
        Frame {
            origin: rig.body,
            pos: self.body.point(pos),
            rot: self.body.rot * rot,
        }
        .placed()
    }

    /// Leg angles lying down: forelegs folded under the chest, hind legs tucked to the side.
    fn fold(&self, front: bool) -> [f32; 4] {
        if self.deer {
            if front {
                [2.6, -1.35, 1.75, 2.6]
            } else {
                [-2.05, 1.45, -1.55, -1.2]
            }
        } else if front {
            [2.75, -1.45, -1.6, -1.55]
        } else {
            [-2.0, 1.5, -1.5, -1.5]
        }
    }
}

fn flip(p: Vec3) -> Vec3 {
    Vec3::new(-p.x, p.y, p.z)
}

// ======================================================================= rabbits

fn rabbit(piece: Piece, c: &Ctx) -> Placed {
    let r = &RABBIT;
    let st = c.state;
    let moving = smooth(0.1, 0.35, c.speed);
    let fast = smooth(1.8, 2.6, c.speed);
    // Hop cycle: distance-based when slow, a steady beat when running.
    let t = if fast > 0.5 {
        (c.now * 3.4).fract()
    } else {
        c.phase / TAU
    };
    let hop_h = (0.035 + 0.07 * smooth(0.3, 5.0, c.speed)) * moving;
    let air = smooth(0.12, 0.25, t) * (1.0 - smooth(0.62, 0.78, t));
    let arc = ((t - 0.15) / 0.62).clamp(0.0, 1.0);
    let lift = (PI * arc).sin() * hop_h;
    let pitch_hop = (0.22 * (1.0 - smooth(0.15, 0.45, t)) * smooth(0.0, 0.15, t)
        - 0.32 * smooth(0.45, 0.65, t) * (1.0 - smooth(0.7, 0.95, t)))
        * moving;
    let dead = if st == AnimalState::Dead {
        (c.t / 0.5).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let sit_up = if st == AnimalState::Alert {
        smooth(0.0, 0.35, c.t) * (1.0 - moving)
    } else {
        0.0
    };
    let rest = if st == AnimalState::Rest {
        smooth(0.0, 1.0, c.t)
    } else {
        0.0
    };
    let graze = if matches!(st, AnimalState::Graze | AnimalState::Drink) {
        smooth(0.0, 0.5, c.t) * (1.0 - moving)
    } else {
        0.0
    };
    // Body: hops, sits up on its haunches, flattens to rest.
    let mut pos = r.body + Vec3::Y * (lift - 0.018 * rest - 0.008 * graze);
    let mut pitch = pitch_hop + 0.95 * sit_up + 0.08 * graze;
    pos += Vec3::new(0.0, 0.05, 0.035) * sit_up;
    pos.y += 0.002 * (c.now * 4.0 + c.seed * 9.0).sin();
    let mut rot = Quat::from_rotation_x(pitch);
    if dead > 0.0 {
        let side = if c.seed > 0.5 { 1.0 } else { -1.0 };
        pos.y = pos.y + (0.045 - pos.y) * dead;
        rot = Quat::from_rotation_z(side * FRAC_PI_2 * dead * dead) * rot;
        pitch = 0.0;
    }
    let _ = pitch;
    let body = Frame {
        origin: r.body,
        pos,
        rot,
    };
    let side_of = |k: u8| if k.is_multiple_of(2) { -1.0 } else { 1.0 };
    match piece {
        Piece::Body => body.placed(),
        Piece::Head | Piece::Gloss | Piece::Ear(_) => {
            let nibble = (c.now * 9.0).sin() * 0.04 * graze;
            let look = 0.4 * drift(c.now, 0.5, c.seed) * (1.0 - moving);
            let head_pitch = -0.55 * graze + nibble - 0.85 * sit_up + 0.2 * moving * (1.0 - fast)
                - 0.15 * fast
                + 0.1 * rest;
            let head = body.child(
                r.head,
                Quat::from_rotation_y(look) * Quat::from_rotation_x(head_pitch),
            );
            match piece {
                Piece::Ear(s) => {
                    let side = side_of(s);
                    let flat = rest.max(fast * 0.8).max(dead);
                    let back = 0.25 + 0.9 * flat - 0.2 * sit_up
                        + 0.15 * twitch(c.now, 1.9, c.seed + s as f32 * 0.4);
                    let spread = 0.12 + 0.15 * graze - 0.08 * sit_up;
                    let pivot = if s == 0 { flip(r.ear) } else { r.ear };
                    head.child(
                        pivot,
                        Quat::from_rotation_z(-side * spread) * Quat::from_rotation_x(back),
                    )
                    .placed()
                }
                _ => head.placed(),
            }
        }
        Piece::Tail => {
            let up = 0.9 * smooth(1.0, 3.0, c.speed) + 0.4 * sit_up;
            body.child(r.tail, Quat::from_rotation_x(-up)).placed()
        }
        Piece::Leg(k, s) => {
            let side = side_of(k);
            let front = k < 2;
            let j = if front { r.front } else { r.hind };
            let j = if side < 0.0 { j.map(flip) } else { j };
            let (mut ang, mut foot) = if front {
                // Reach forward in the air, push back on landing; dangle when sitting up.
                (
                    (-0.7 * air + 0.45 * smooth(0.6, 0.85, t) * (1.0 - smooth(0.9, 1.0, t)))
                        * moving
                        - 0.9 * sit_up,
                    0.6 * sit_up + 0.4 * air * moving,
                )
            } else {
                // Push off and trail behind, then swing forward under the body to land.
                let push = smooth(0.0, 0.3, t) * (1.0 - smooth(0.5, 0.8, t));
                let tuck = smooth(0.55, 0.85, t) * (1.0 - smooth(0.92, 1.0, t));
                (
                    (0.9 * push - 0.55 * tuck) * moving - 0.95 * sit_up,
                    (-1.1 * push + 0.4 * tuck) * moving + 0.95 * sit_up,
                )
            };
            if dead > 0.0 {
                ang += (if front { -0.5 } else { 0.6 } - ang) * dead;
                foot *= 1.0 - dead;
            }
            let upper = body.child(j[0], Quat::from_rotation_x(ang));
            if s == 0 {
                upper.placed()
            } else {
                upper.child(j[1], Quat::from_rotation_x(foot)).placed()
            }
        }
        _ => HIDDEN,
    }
}

// ======================================================================= birds

fn bird(piece: Piece, c: &Ctx) -> Placed {
    let b = &BIRD;
    let st = c.state;
    let flying = st == AnimalState::Flying;
    let dead = st == AnimalState::Dead;
    let vy = c.loco.velocity.y;
    // On the ground: hops lift it (the flight code moves the body), pecks tip it forward.
    let hopping = !flying && vy.abs() > 0.05;
    let peck_t = (c.now * 1.7 + c.seed * 5.0).fract();
    let pecking = !flying && !hopping && !dead && (c.now * 0.31 + c.seed * 3.0).sin() > -0.2;
    let peck = if pecking {
        smooth(0.0, 0.12, peck_t) * (1.0 - smooth(0.2, 0.38, peck_t))
    } else {
        0.0
    };
    let takeoff = if flying {
        1.0 - smooth(0.25, 0.6, c.t)
    } else {
        0.0
    };
    let glide = if flying {
        smooth(-0.3, -1.2, vy) * (1.0 - takeoff)
    } else {
        0.0
    };
    let mut pos = b.body;
    let mut pitch = -0.35 * peck + if hopping { 0.12 } else { 0.0 };
    if flying {
        pos.y += 0.02;
        pitch = 0.15 * takeoff;
    }
    let mut rot = Quat::from_rotation_x(pitch);
    if dead {
        let f = (c.t / 0.3).clamp(0.0, 1.0);
        pos.y = pos.y + (0.03 - pos.y) * f;
        rot = Quat::from_rotation_z(FRAC_PI_2 * f) * rot;
    }
    let body = Frame {
        origin: b.body,
        pos,
        rot,
    };
    // Quick head turns between still moments, like a real bird.
    let turn = ((c.now * 1.3 + c.seed * 11.0).floor() * 2.39).sin() * 0.6;
    let head = body.child(
        b.head,
        Quat::from_rotation_y(if flying || dead { 0.0 } else { turn })
            * Quat::from_rotation_x(-0.9 * peck + if flying { 0.25 } else { 0.0 }),
    );
    let wing_frame = |s: u8, hand: bool| -> Placed {
        let side = if s == 0 { -1.0 } else { 1.0 };
        let (shoulder, wrist) = if s == 0 {
            (flip(b.shoulder), flip(b.wrist))
        } else {
            (b.shoulder, b.wrist)
        };
        // Flap: raised on the upstroke with the hand folding back, extended on the downstroke.
        let f = c.flap;
        let amp = 1.0 + 0.25 * takeoff - 0.75 * glide;
        let lift = (f.sin() * 0.85 + 0.15) * amp + 0.12 * glide;
        let up = (f.cos()).max(0.0) * (1.0 - glide);
        let sweep = 0.15 + 0.35 * up;
        let arm = Quat::from_rotation_z(side * lift) * Quat::from_rotation_y(-side * sweep * 0.5);
        let a = body.child(shoulder, arm);
        if !hand {
            return a.placed();
        }
        let hand_rot = Quat::from_rotation_y(-side * (0.2 + 0.9 * up))
            * Quat::from_rotation_z(side * 0.25 * up);
        a.child(wrist, hand_rot).placed()
    };
    let hide = |p: Placed, show: bool| if show { p } else { HIDDEN };
    match piece {
        Piece::Body => body.placed(),
        Piece::Head | Piece::Gloss => head.placed(),
        Piece::Tail => hide(
            body.child(
                b.tail,
                Quat::from_rotation_x(0.25 * twitch(c.now, 1.3, c.seed) - 0.3 * peck),
            )
            .placed(),
            !flying,
        ),
        Piece::TailFan => hide(
            body.child(
                b.tail,
                Quat::from_rotation_x(-0.15 * glide + 0.1 * (c.flap).sin()),
            )
            .placed(),
            flying,
        ),
        Piece::Wing(_, 2) => hide(body.placed(), !flying),
        Piece::Wing(s, w) => hide(wing_frame(s, w == 1), flying),
        Piece::Leg(s, _) => {
            let hip = if s == 0 { flip(b.hip) } else { b.hip };
            let tuck = if flying {
                1.0 - takeoff * 0.7
            } else if hopping {
                0.4
            } else {
                0.0
            };
            hide(
                body.child(hip, Quat::from_rotation_x(1.3 * tuck)).placed(),
                !flying || takeoff > 0.2 || glide > 0.8,
            )
        }
        _ => HIDDEN,
    }
}

// ======================================================================= stand-ins

/// The primitive stand-ins, for species whose sculpted bodies are not made yet.
mod stand_in {
    use std::f32::consts::FRAC_PI_2;

    use engine_core::prelude::*;
    use engine_render::{MeshRenderer, Shape, StandardMaterial};
    use engine_world::locomotion::{LocomotionMode, LocomotionState};

    use super::super::*;
    use crate::wildlife::{Animal, AnimalState, Species};

    /// The parts of an animal's body.
    pub(super) fn parts(
        stand_in: &StandIn,
        animal: Option<&Animal>,
        species: Species,
    ) -> Vec<(Part, MeshRenderer, Vec3)> {
        let mut parts: Vec<(Part, Shape, StandardMaterial, Vec3)> = Vec::new();
        let one = Vec3::ONE;
        let capsule = |radius: f32, height: f32| Shape::Capsule {
            radius,
            height,
            segments: 12,
        };
        let sphere = |radius: f32| Shape::Sphere {
            radius,
            segments: 16,
        };
        let cone = |radius: f32, height: f32| Shape::Cone {
            radius,
            height,
            segments: 10,
        };
        let cuboid = |x: f32, y: f32, z: f32| Shape::Cuboid { size: [x, y, z] };
        let body = mat(stand_in.color.unwrap_or_else(|| species_color(species)));
        let pale = mat(0xe8e2d6);
        let dark = mat(0x2a2522);
        match species {
            Species::Deer => {
                parts.push((Part::Body, capsule(0.27, 1.3), body, one));
                parts.push((Part::Neck, capsule(0.09, 0.65), body, one));
                parts.push((Part::Head, capsule(0.09, 0.36), body, one));
                parts.push((Part::Snout, sphere(0.05), dark, one));
                for k in 0..4 {
                    parts.push((Part::Leg(k), capsule(0.045, 0.95), body, one));
                }
                parts.push((Part::Tail, sphere(0.07), pale, Vec3::new(1.0, 1.3, 0.7)));
                for k in 0..2 {
                    parts.push((Part::Ear(k), cone(0.05, 0.16), body, one));
                }
                if animal.is_some_and(|a| a.male) {
                    for k in 0..2 {
                        parts.push((Part::Antler(k), cone(0.03, 0.45), mat(0xd9c9a3), one));
                    }
                }
            }
            Species::Wolf => {
                parts.push((Part::Body, capsule(0.18, 1.0), body, one));
                parts.push((Part::Neck, capsule(0.1, 0.34), body, one));
                parts.push((Part::Head, sphere(0.12), body, one));
                parts.push((Part::Snout, cone(0.06, 0.2), dark, one));
                for k in 0..4 {
                    parts.push((Part::Leg(k), capsule(0.04, 0.6), body, one));
                }
                parts.push((Part::Tail, capsule(0.05, 0.5), body, one));
                for k in 0..2 {
                    parts.push((Part::Ear(k), cone(0.045, 0.12), dark, one));
                }
            }
            Species::Rabbit => {
                parts.push((Part::Body, sphere(0.14), body, Vec3::new(0.9, 0.85, 1.25)));
                parts.push((Part::Head, sphere(0.075), body, one));
                for k in 0..2 {
                    parts.push((Part::Ear(k), capsule(0.022, 0.17), body, one));
                }
                parts.push((Part::Tail, sphere(0.04), pale, one));
            }
            Species::Bird => {
                parts.push((Part::Body, sphere(0.07), body, Vec3::new(0.85, 0.8, 1.5)));
                parts.push((Part::Head, sphere(0.045), body, one));
                parts.push((Part::Snout, cone(0.015, 0.05), mat(0xe0a030), one));
                for k in 0..2 {
                    parts.push((Part::Wing(k), cuboid(0.24, 0.012, 0.1), body, one));
                }
            }
        }
        parts
            .into_iter()
            .map(|(part, shape, material, scale)| (part, MeshRenderer::new(shape, material), scale))
            .collect()
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn pose(
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
        let moving = speed > 0.2;
        let head_down = matches!(
            state,
            AnimalState::Graze | AnimalState::Drink | AnimalState::Feed
        ) && !moving;
        let alert = state == AnimalState::Alert;
        let resting = state == AnimalState::Rest;
        let dead = state == AnimalState::Dead;
        let low = state == AnimalState::Stalk || loco.mode == LocomotionMode::Crouch;
        let galloping = loco.mode == LocomotionMode::Sprint || speed > 5.0;
        let fall = if dead {
            ((now - since) / 0.6).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };
        let roll = Quat::from_rotation_z(fall * FRAC_PI_2);
        let along = Quat::from_rotation_x(FRAC_PI_2);
        let leg_swing = |k: u8| -> f32 {
            if !moving {
                return 0.0;
            }
            let amp = (speed / 6.0).clamp(0.25, 0.9);
            let front = k < 2;
            let left = k.is_multiple_of(2);
            let offset = if galloping {
                if front { 0.0 } else { PI * 0.8 }
            } else if front == left {
                0.0
            } else {
                PI
            };
            (phase + offset + if galloping && left { 0.25 } else { 0.0 }).sin() * amp
        };
        let place = |p: Placed| -> Placed {
            if fall <= 0.0 {
                return p;
            }
            Placed {
                pos: roll * p.pos + Vec3::Y * 0.0,
                rot: roll * p.rot,
                visible: p.visible,
            }
        };
        match species {
            Species::Deer | Species::Wolf => {
                let deer = species == Species::Deer;
                let (body_y, body_len, leg_len, half_x, half_z) = if deer {
                    (1.0, 1.3, 0.95, 0.14, 0.42)
                } else {
                    (0.62, 1.0, 0.6, 0.11, 0.33)
                };
                let mut y = body_y;
                if resting {
                    y = if deer { 0.32 } else { 0.22 };
                } else if low {
                    y -= 0.18;
                } else if dead {
                    y = if deer { 0.3 } else { 0.2 };
                }
                if galloping {
                    y += (phase * 2.0).sin().abs() * 0.08;
                }
                let pitch = if galloping { (phase).sin() * 0.06 } else { 0.0 };
                let body_rot = Quat::from_rotation_x(pitch);
                let body = Vec3::new(0.0, y, 0.0);
                let front = body + body_rot * Vec3::new(0.0, 0.08, -body_len * 0.38);
                // Neck direction: up-forward, straight up when alert, down to the grass.
                let neck_dir = if head_down {
                    Vec3::new(0.0, -0.75, -0.65)
                } else if alert {
                    Vec3::new(0.0, 0.97, -0.25)
                } else if low || (!deer && moving) {
                    Vec3::new(0.0, 0.25, -0.97)
                } else if deer {
                    Vec3::new(0.0, 0.78, -0.62)
                } else {
                    Vec3::new(0.0, 0.45, -0.9)
                }
                .normalize();
                let neck_len = if deer { 0.6 } else { 0.3 };
                let neck_rot = Quat::from_rotation_arc(Vec3::Y, neck_dir);
                let neck_end = front + neck_dir * neck_len;
                let lunge = if biting {
                    Vec3::new(0.0, -0.05, -0.25)
                } else {
                    Vec3::ZERO
                };
                let head = neck_end + lunge;
                let face = if head_down {
                    Vec3::new(0.0, -0.9, -0.45)
                } else {
                    Vec3::new(0.0, -0.2, -1.0)
                }
                .normalize();
                let head_rot = Quat::from_rotation_arc(Vec3::Y, face);
                let p = match part {
                    Part::Body => at(body, body_rot * along),
                    Part::Neck => at(front + neck_dir * neck_len * 0.5, neck_rot),
                    Part::Head => {
                        if deer {
                            at(head + face * 0.12, head_rot)
                        } else {
                            at(head, Quat::IDENTITY)
                        }
                    }
                    Part::Snout => {
                        if deer {
                            at(head + face * 0.3, Quat::IDENTITY)
                        } else {
                            at(head + face * 0.17, head_rot)
                        }
                    }
                    Part::Leg(k) => {
                        let side = if k % 2 == 0 { -1.0 } else { 1.0 };
                        let z = if k < 2 { -half_z } else { half_z };
                        let hip = body + body_rot * Vec3::new(side * half_x, -0.05, z);
                        let rot = if resting || dead {
                            Quat::from_rotation_x(if k < 2 { 1.45 } else { -1.45 })
                        } else {
                            Quat::from_rotation_x(leg_swing(k) + if low { 0.35 } else { 0.0 })
                        };
                        limb(hip, rot, leg_len)
                    }
                    Part::Tail => {
                        let back = body + body_rot * Vec3::new(0.0, 0.1, body_len * 0.45);
                        if deer {
                            // Flagging white tail when alarmed.
                            let up = matches!(state, AnimalState::Alert | AnimalState::Flee);
                            at(
                                back + if up {
                                    Vec3::new(0.0, 0.12, 0.03)
                                } else {
                                    Vec3::new(0.0, -0.05, 0.05)
                                },
                                Quat::IDENTITY,
                            )
                        } else {
                            let droop = if state == AnimalState::Chase {
                                1.4
                            } else if low {
                                2.4
                            } else {
                                2.0
                            };
                            let r = Quat::from_rotation_x(droop);
                            at(back + r * Vec3::new(0.0, 0.25, 0.0), r)
                        }
                    }
                    Part::Ear(k) => {
                        let side = if k == 0 { -1.0 } else { 1.0 };
                        let up = if head_down { face } else { Vec3::Y };
                        let base = head + Vec3::new(side * 0.07, 0.0, 0.0) + up * 0.08;
                        let tilt = Quat::from_rotation_z(-side * if alert { 0.1 } else { 0.5 });
                        at(base + Vec3::new(0.0, 0.06, 0.0), tilt)
                    }
                    Part::Antler(k) => {
                        if !male {
                            return HIDDEN;
                        }
                        let side = if k == 0 { -1.0 } else { 1.0 };
                        let up = if head_down { face } else { Vec3::Y };
                        let r = Quat::from_rotation_z(-side * 0.45) * Quat::from_rotation_x(0.25);
                        at(head + Vec3::new(side * 0.06, 0.0, 0.0) + up * 0.25, r)
                    }
                    _ => HIDDEN,
                };
                place(p)
            }
            Species::Rabbit => {
                let hop = if moving {
                    (phase).sin().abs() * 0.14
                } else {
                    0.0
                };
                let y = 0.13 + hop - if resting { 0.03 } else { 0.0 };
                let sit_up = alert && !moving;
                let body = Vec3::new(0.0, y + if sit_up { 0.04 } else { 0.0 }, 0.0);
                let body_rot = if sit_up {
                    Quat::from_rotation_x(0.9)
                } else {
                    Quat::IDENTITY
                };
                let head = body
                    + if sit_up {
                        Vec3::new(0.0, 0.17, -0.04)
                    } else {
                        Vec3::new(0.0, 0.07, -0.16)
                    };
                let p = match part {
                    Part::Body => at(body, body_rot),
                    Part::Head => at(head, Quat::IDENTITY),
                    Part::Ear(k) => {
                        let side = if k == 0 { -1.0 } else { 1.0 };
                        let back = if moving || resting { 0.9 } else { 0.15 };
                        let r = Quat::from_rotation_x(back) * Quat::from_rotation_z(-side * 0.15);
                        at(
                            head + Vec3::new(side * 0.03, 0.05, 0.0)
                                + r * Vec3::new(0.0, 0.08, 0.0),
                            r,
                        )
                    }
                    Part::Tail => at(body + body_rot * Vec3::new(0.0, 0.03, 0.17), Quat::IDENTITY),
                    _ => HIDDEN,
                };
                place(p)
            }
            Species::Bird => {
                let flying = state == AnimalState::Flying;
                let peck = !flying && ((now * 2.3 + since).sin() > 0.6);
                let body = Vec3::new(0.0, if flying { 0.0 } else { 0.07 }, 0.0);
                let head = body
                    + if peck {
                        Vec3::new(0.0, -0.02, -0.12)
                    } else {
                        Vec3::new(0.0, 0.06, -0.09)
                    };
                let p = match part {
                    Part::Body => at(body, Quat::IDENTITY),
                    Part::Head => at(head, Quat::IDENTITY),
                    Part::Snout => at(
                        head + Vec3::new(0.0, 0.0, -0.05),
                        Quat::from_rotation_x(-FRAC_PI_2),
                    ),
                    Part::Wing(k) => {
                        let side = if k == 0 { -1.0 } else { 1.0 };
                        let a = if flying { flap.sin() * 0.9 } else { -0.15 };
                        let r = Quat::from_rotation_z(side * a);
                        let reach = if flying { 0.13 } else { 0.06 };
                        at(body + r * Vec3::new(side * reach, 0.02, 0.0), r)
                    }
                    _ => HIDDEN,
                };
                place(p)
            }
        }
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
        sculpted_pose(
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
        if species == Species::Deer {
            &DEER
        } else {
            &WOLF
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
        for species in [Species::Deer, Species::Wolf] {
            for k in 0..4 {
                let t = toe(species, k, AnimalState::Wander, 0.0, 0.0, 100.0);
                assert!(t.y.abs() < 0.01, "{species:?} leg {k}: toe at {t}");
            }
        }
    }

    #[test]
    fn moving_feet_touch_the_ground_and_never_sink() {
        for species in [Species::Deer, Species::Wolf] {
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
        for species in [Species::Deer, Species::Wolf] {
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
}
