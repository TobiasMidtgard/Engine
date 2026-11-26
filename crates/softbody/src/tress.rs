//! Scalp hair in motion: the guide-strand solver of the owner's `gta-hair-sim` project (its
//! `shaders/hair_sim.wgsl`, pass `sim_strands`), ported to the CPU and to people with whole
//! skeletons.
//!
//! Every guide is a chain of up to [`MAX_VERTS`] points about two centimeters apart, rooted on
//! the bone frame it rides. Each substep a strand
//!
//! 1. pins its root to the frame, and spreads its points' velocities along itself from there
//!    (friction inside the lock, [`TressConfig::inner_friction`]: not in that project),
//! 2. predicts its points (Verlet) under gravity, the air (wind with gusts and curl-noise
//!    turbulence, drag that is weaker along the strand than across it), a cushion of pressure
//!    where hair is dense and a pull toward the hair's mean velocity there (a voxel grid),
//! 3. pulls its first points softly back to where the head carries them,
//! 4. turns every segment toward its grown direction relative to the segment before it (local
//!    shape matching: waves and the comb's direction survive, and swing),
//! 5. keeps its bends and its length (symmetric distance constraints, then follow-the-leader)
//!    and slides off the head, ears, neck, shoulders and torso ([`TressCapsule`]: tapered
//!    capsules on the bones, the creases between the body's blended smooth),
//! 6. ends on a projection that leaves every segment its exact length outside every capsule,
//! 7. and takes its velocities from where the points ended up.
//!
//! The constants are that project's. What a person in a world adds: the capsules ride bones
//! instead of one head; the hair's speed limit, damping and drag are measured against the head
//! it rides, so a run or a ride leaves the hair streaming, not torn; a dash's jolt is partly
//! followed; the wind is the weather's, in m/s; a strand that lies still sleeps; and a lock
//! has friction inside it, where that project's swings like a bare chain.

use engine_core::glam::Mat3A;
use engine_core::prelude::*;

use crate::solver::{Sim, leaf_pool};

/// Most points a strand is simulated with.
pub const MAX_VERTS: usize = 32;
/// Most capsules hair slides off (the first that many of [`Tress::capsules`]).
pub const MAX_CAPSULES: usize = 32;
/// Most boxes of the body's measured surface ([`Tress::boxes`]).
pub const MAX_BOXES: usize = 16;
/// Fastest a point moves against the head it rides, m/s.
const MAX_SPEED: f32 = 7.5;
/// The voxel grid's cell, m, and its most cells along an axis.
const CELL: f32 = 0.015;
const GRID_MAX: usize = 64;
/// Guide points the source project simulates (1792 guides of 20): its pressure and cohesion
/// constants are for that many in a head of hair, so fewer points each weigh more.
const SOURCE_POINTS: f32 = 35_840.0;
const SOURCE_STRANDS: f32 = 1792.0;
/// How much of a jolt of the head (a dash, a teleport-like jerk: hundreds of m/s²) the hair
/// follows rigidly. Less, and the head sweeps through the hair on its leading side within a
/// step, which comes out of the capsules wherever it is nearest: folded up beside the ear
/// after a 30 cm jerk at 0.6.
const FOLLOWED: f32 = 0.85;
/// Seconds over which the head's velocity is taken.
const CARRIED_OVER: f32 = 0.1;
/// Width of the fillet in the creases between the body's capsules, m.
const FILLET: f32 = 0.005;
/// Furthest the constraint passes of one substep move a point, m: capsules further off than
/// this when they begin aren't looked at again in them.
const REACHED: f32 = 0.03;
/// A box reads the body at a point only where its bone carries at least this much of the
/// share the box that carries most there does, and the bones are blended this many times for
/// where the point was ([`body_at`]; as the character module's reader and the strand
/// renderer's do).
pub const READS_FROM: f32 = 0.25;
const REFINED: usize = 2;
/// A bone that carries this much of the skin carries it all: no other is asked.
const CARRIES_ALL: f32 = 0.98;
/// A strand none of whose points moves faster than [`RESTFUL`] (m/s, against the head) for
/// this long, s, sleeps: it rides its bone until the body or the air moves. The pull to the
/// grown shape acts on positions and holds no energy, so a lock lying on a shoulder creeps
/// up it for good at under a centimeter a second, falls and starts again; asleep it lies.
const SLEEPS_AFTER: f32 = 0.5;
const RESTFUL: f32 = 0.03;
/// What wakes the hair: air past the head faster than this, m/s, a bone further from where
/// it was against the head than this, m, or turned further (the cosine of half the angle,
/// 0.6 degrees), the head's own turn included.
const STILL_AIR: f32 = 0.02;
const STILL_BONE: f32 = 0.002;
const STILL_TURN: f32 = 0.999_986;

/// Soaked hair against dry: how much less its grown shape holds, how much less the air moves
/// it, how much of its volume (the hair's pressure, and the clearance it keeps from the body)
/// it loses, how much more it moves as one (cohesion, and the friction inside a lock, as
/// factors on top of 1), and the friction it adds on what it lies on.
const WET_LIMP: f32 = 0.5;
const WET_HEAVY: f32 = 0.6;
const WET_FLAT: f32 = 0.7;
const WET_GATHERS: f32 = 2.0;
const WET_CLINGS: f32 = 0.4;
/// Hair on skin and cloth grips: pressed on by `d` in a substep it slides less than
/// [`STICKS`]` * d` not at all, and further by [`SLIDES`]` * d` less (Coulomb's friction, in
/// positions), at most [`GRIPS_MOST`] m a substep. Without it a lock lying on a shoulder never
/// rests: its grown shape draws it over the shoulder, it falls back and starts again.
const STICKS: f32 = 0.8;
const SLIDES: f32 = 0.55;
const GRIPS_MOST: f32 = 0.0004;
/// Of a point's slide along the body, the contact itself takes at most this times how far the
/// point was pressed in.
const RUBS: f32 = 0.3;
/// Soaked hair lies on the body as if pressed in this far (m) more: the water between them
/// holds it there.
const WET_HOLDS: f32 = 0.0002;

/// Seconds a piece of hair cut off is simulated and drawn for: it falls, lies where it fell,
/// and is gone.
pub const SHED_SECS: f32 = 4.0;
/// A falling piece's drag, 1/s: clippings sink through still air at about a meter and a half
/// a second.
const PIECE_DRAG: f32 = 6.5;

/// Tuning (the source project's `SimConfig` defaults).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TressConfig {
    /// Substeps per 1/60 s.
    pub substeps: u32,
    /// Constraint iterations per substep; the last is follow-the-leader.
    pub iterations: u32,
    pub air_drag: f32,
    /// How much weaker the air drags along a strand than across it.
    pub aniso: f32,
    /// Depth of the gusts' swell, 0..1.
    pub gust: f32,
    pub turbulence: f32,
    pub turbulence_freq: f32,
    /// Scales every point's shape and bend stiffness.
    pub stiffness: f32,
    pub damping: f32,
    pub friction: f32,
    /// Strength of the hair's pressure against itself.
    pub self_collision: f32,
    /// How much a point takes on the mean velocity of the hair around it.
    pub cohesion: f32,
    /// Friction inside a lock, m^2/s: how fast a difference in velocity spreads along a
    /// strand. The source has none, and its hair swings like a chain: a head shaking at the
    /// rhythm a 30 cm lock swings at by itself (about 1.1 Hz) threw the hair out level with
    /// the ears and across the throat. With it the hair below takes up what the hair above
    /// does, a real lock's hairs rubbing on each other, and a shake swings it half as far.
    pub inner_friction: f32,
    /// Wind strength (the source project's unit, 0..6) per m/s of wind at the head: hair
    /// against a head feels less than the open air.
    pub wind_feel: f32,
}

impl Default for TressConfig {
    fn default() -> Self {
        Self {
            substeps: 6,
            iterations: 3,
            air_drag: 1.4,
            aniso: 3.0,
            gust: 0.35,
            turbulence: 0.65,
            turbulence_freq: 1.8,
            stiffness: 1.0,
            damping: 0.035,
            friction: 0.32,
            self_collision: 0.75,
            cohesion: 0.35,
            inner_friction: 0.1,
            wind_feel: 0.5,
        }
    }
}

/// One simulated point of a strand (particle `i` of the [`Sim`] is `verts[i]`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TressVertex {
    /// As grown, in the strand's frame.
    pub rest: Vec3,
    /// Length of the segment to the point before, m.
    pub rest_len: f32,
    /// 0 at the root .. 1 at the tip.
    pub t: f32,
    pub stiffness: f32,
    pub bend: f32,
    /// How firmly the head carries the point, 0..1.
    pub pin: f32,
    pub damping: f32,
    /// How deep inside the head's and the body's capsules the point was grown, m: it may sit
    /// that deep again (the capsules stand in for a body they are a little fatter than in
    /// places; hair grown lying on that body isn't pushed off it).
    pub sunk: [f32; 2],
}

/// One strand: particles `first..first + count`, root first, riding solver frame `frame`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TressStrand {
    pub first: u32,
    pub count: u32,
    pub frame: u16,
    /// 0..1, the strand's own phase in the gusts.
    pub seed: f32,
}

/// A strand cut through: its particles from `at` on (strand-relative, never the root; 0: the
/// strand is whole) are no longer part of it. Those up to `end` are the piece that last came
/// off, falling free until `shed` reaches 1; the rest came off before and are gone.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TressCut {
    pub at: u16,
    pub end: u16,
    /// How far along the segment into particle `at` the cut is, 0..1.
    pub part: f32,
    /// 0 when the piece came off .. 1 gone ([`SHED_SECS`] later).
    pub shed: f32,
}

/// A tapered capsule on a bone that hair slides off: ends and radii in solver frame `frame`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TressCapsule {
    pub frame: u16,
    pub a: Vec3,
    pub ra: f32,
    pub b: Vec3,
    pub rb: f32,
    pub friction: f32,
    /// Neck, shoulders and torso (their creases are blended smooth); not the head and ears.
    pub body: bool,
}

/// One bone's box of the body as its look module measured it
/// ([`engine_world::HairBody`]): the signed distance to the dressed skin at `size` samples
/// `cell` apart from `min`, in solver frame `frame`, and how much the frame's bone carries the
/// skin nearest each sample.
#[derive(Debug, Clone, PartialEq)]
pub struct TressBox {
    pub frame: u16,
    pub min: Vec3,
    pub cell: f32,
    pub size: [u16; 3],
    /// Per sample, x fastest: the distance in [`engine_world::HairBody::UNIT`]s, and the
    /// weight (0..=255).
    pub cells: std::sync::Arc<[[i16; 2]]>,
}

impl TressBox {
    /// A [`engine_world::HairBodyPart`]'s samples for the solver (`None` when some are
    /// missing).
    pub fn new(frame: u16, part: &engine_world::HairBodyPart) -> Option<Self> {
        let n = part.size.iter().map(|s| *s as usize).product::<usize>();
        if part.size.iter().any(|s| *s < 2)
            || part.cell <= 0.0
            || part.distance.len() != n
            || part.weight.len() != n
        {
            return None;
        }
        Some(Self {
            frame,
            min: part.min,
            cell: part.cell,
            size: part.size,
            cells: part
                .distance
                .iter()
                .zip(part.weight.iter())
                .map(|(d, w)| [*d, *w as i16])
                .collect(),
        })
    }

    /// Whether the samples are all there.
    pub fn whole(&self) -> bool {
        let n = self.size.iter().map(|s| *s as usize).product::<usize>();
        self.size.iter().all(|s| *s >= 2) && self.cell > 0.0 && self.cells.len() == n
    }

    /// Whether `p` in the box's frame is in it or within `pad` of it.
    #[inline]
    fn holds(&self, p: Vec3A, pad: f32) -> bool {
        let lo = Vec3A::from(self.min) - Vec3A::splat(pad);
        let hi = Vec3A::from(self.min)
            + Vec3A::new(
                (self.size[0] - 1) as f32,
                (self.size[1] - 1) as f32,
                (self.size[2] - 1) as f32,
            ) * self.cell
            + Vec3A::splat(pad);
        p.cmpge(lo).all() && p.cmple(hi).all()
    }

    /// At `p` in the box's frame: distance (m) and the bone's weight (0..1).
    pub fn distance_at(&self, p: Vec3) -> Option<(f32, f32)> {
        self.at(p.into()).map(|(d, _, w)| (d, w))
    }

    /// At `p` in the box's frame: how much of the skin nearest there its bone carries (0..1).
    #[inline]
    fn share_at(&self, p: Vec3A) -> Option<f32> {
        let c = (p - Vec3A::from(self.min)) / self.cell;
        let [sx, sy, sz] = self.size.map(|s| s as usize);
        if c.cmplt(Vec3A::ZERO).any()
            || c.x >= (sx - 1) as f32
            || c.y >= (sy - 1) as f32
            || c.z >= (sz - 1) as f32
        {
            return None;
        }
        let (x, y, z) = (c.x as usize, c.y as usize, c.z as usize);
        let f = c - Vec3A::new(x as f32, y as f32, z as f32);
        let k = x + sx * (y + sy * z);
        let row = sx * sy;
        let w = |i: usize| self.cells[i][1] as f32;
        let mix = |a: f32, b: f32, t: f32| a + (b - a) * t;
        Some(
            mix(
                mix(
                    mix(w(k), w(k + 1), f.x),
                    mix(w(k + sx), w(k + sx + 1), f.x),
                    f.y,
                ),
                mix(
                    mix(w(k + row), w(k + row + 1), f.x),
                    mix(w(k + row + sx), w(k + row + sx + 1), f.x),
                    f.y,
                ),
                f.z,
            ) / 255.0,
        )
    }

    /// At `p` in the box's frame: distance (m), its gradient, and how much of the skin nearest
    /// there its bone carries (0..1).
    #[inline]
    fn at(&self, p: Vec3A) -> Option<(f32, Vec3A, f32)> {
        let c = (p - Vec3A::from(self.min)) / self.cell;
        let [sx, sy, sz] = self.size.map(|s| s as usize);
        if c.cmplt(Vec3A::ZERO).any()
            || c.x >= (sx - 1) as f32
            || c.y >= (sy - 1) as f32
            || c.z >= (sz - 1) as f32
        {
            return None;
        }
        let (x, y, z) = (c.x as usize, c.y as usize, c.z as usize);
        let f = c - Vec3A::new(x as f32, y as f32, z as f32);
        let k = x + sx * (y + sy * z);
        let row = sx * sy;
        let ids = [
            k,
            k + 1,
            k + sx,
            k + sx + 1,
            k + row,
            k + row + 1,
            k + row + sx,
            k + row + sx + 1,
        ];
        let cells = ids.map(|i| self.cells[i]);
        let d = cells.map(|c| c[0] as f32);
        let w = cells.map(|c| c[1] as f32);
        let mix = |a: f32, b: f32, t: f32| a + (b - a) * t;
        let x00 = mix(d[0], d[1], f.x);
        let x10 = mix(d[2], d[3], f.x);
        let x01 = mix(d[4], d[5], f.x);
        let x11 = mix(d[6], d[7], f.x);
        let y0 = mix(x00, x10, f.y);
        let y1 = mix(x01, x11, f.y);
        let gx = mix(
            mix(d[1] - d[0], d[3] - d[2], f.y),
            mix(d[5] - d[4], d[7] - d[6], f.y),
            f.z,
        );
        let weight = mix(
            mix(mix(w[0], w[1], f.x), mix(w[2], w[3], f.x), f.y),
            mix(mix(w[4], w[5], f.x), mix(w[6], w[7], f.x), f.y),
            f.z,
        );
        const UNIT: f32 = engine_world::HairBody::UNIT;
        Some((
            mix(y0, y1, f.z) * UNIT,
            Vec3A::new(gx, mix(x10 - x00, x11 - x01, f.z), y1 - y0) * (UNIT / self.cell),
            weight / 255.0,
        ))
    }
}

/// A box where it is in one substep.
#[derive(Debug, Clone, Copy, Default)]
struct PosedBox {
    pos: Vec3A,
    rot: Mat3A,
    center: Vec3A,
    reach: f32,
}

/// The measured body at a point: the way out, the signed distance, the box that read it, and
/// how much of the skin there one chosen box's bone carries.
#[derive(Debug, Clone, Copy)]
struct BodyAt {
    out: Vec3A,
    distance: f32,
    part: u8,
    carried: f32,
}

/// How one box sees a point: where the point was on the body as the hair was grown on it,
/// and what brings a small move of the point back there (the bones that carry the skin there,
/// blended by their shares, undone). `carried`: one chosen box's share of that skin.
#[derive(Debug, Clone, Copy, Default)]
struct View {
    part: u8,
    carried: f32,
    was: Vec3A,
    back: Mat3A,
}

/// The boxes that count at a point ([`views`]), each with its [`View`]: at most [`VIEWS`],
/// those whose bones carry most of the skin there.
#[derive(Debug, Clone, Copy, Default)]
struct Views {
    n: u8,
    of: [View; VIEWS],
}

/// How many boxes can count at one point (head, neck, chest and a shoulder meet at a collar).
const VIEWS: usize = 4;
/// Places on the body as grown this near are one place to a reader (a sixteenth of a
/// sample's cell), m.
const SAME: f32 = 0.0005;

/// How the `boxes` in `among`, each where it is `posed`, see `p`, and which of them `p` is in
/// or within `pad` of. `of`: the box whose share of the skin there is wanted (any other
/// number: none).
///
/// Every box holds the whole body as the hair was grown on it, so the question is where `p`
/// was then. Each box answers as its own bone would have brought it, and counts as much as
/// its bone carries of the skin nearest there. Where bones share that skin (a shoulder,
/// between arm and trunk) the skin has moved as their blend by their shares, as it is
/// skinned: that blend undone gives where `p` was, and the body is read there ([`read`]).
/// (Each box holding its own bone's skin alone, the nearest of them the body, threw the hair
/// off a shoulder that was not there: an arm lowered from how the hair was grown swings its
/// box's side of the shoulder up beside the neck.)
#[inline]
fn views(
    boxes: &[TressBox],
    posed: &[PosedBox],
    among: u16,
    p: Vec3A,
    pad: f32,
    of: usize,
) -> (u16, Views) {
    let mut seen = [(0usize, Vec3A::ZERO, 0.0f32); MAX_BOXES];
    let mut n = 0;
    let mut most = 0.0f32;
    let mut within = 0u16;
    let mut left = among;
    while left != 0 {
        let k = left.trailing_zeros() as usize;
        left &= left - 1;
        if k >= boxes.len() {
            break;
        }
        let was = posed[k].rot.transpose() * (p - posed[k].pos);
        if !boxes[k].holds(was, pad) {
            continue;
        }
        within |= 1 << k;
        let Some(share) = boxes[k].share_at(was) else {
            continue;
        };
        seen[n] = (k, was, share);
        n += 1;
        most = most.max(share);
    }
    let mut views = Views::default();
    if most <= 0.0 {
        return (within, views);
    }
    let mut shares = [0.0f32; VIEWS];
    let mut starts = [Vec3A::ZERO; VIEWS];
    for &(k, was, share) in &seen[..n] {
        if share < READS_FROM * most {
            continue;
        }
        // Boxes whose bones have moved alike see the point alike: one of them says it.
        if (0..views.n as usize).any(|v| starts[v].distance_squared(was) < SAME * SAME) {
            continue;
        }
        // Among too many, those whose bones carry most.
        let slot = if (views.n as usize) < VIEWS {
            views.n += 1;
            views.n as usize - 1
        } else {
            let least = (0..VIEWS)
                .min_by(|a, b| shares[*a].total_cmp(&shares[*b]))
                .unwrap_or(0);
            if shares[least] >= share {
                continue;
            }
            least
        };
        shares[slot] = share;
        starts[slot] = was;
        let mut view = View {
            part: k as u8,
            carried: if k == of { 1.0 } else { 0.0 },
            was,
            back: posed[k].rot.transpose(),
        };
        if share < CARRIES_ALL {
            // The bones that carry the skin there, by their shares (and once more where
            // that puts the point: the shares are those of where it really was).
            for _ in 0..REFINED {
                let (mut rot, mut pos, mut total, mut its) = (Mat3A::ZERO, Vec3A::ZERO, 0.0, 0.0);
                for (j, b) in boxes.iter().enumerate() {
                    if let Some(share) = b.share_at(view.was)
                        && share > 0.0
                    {
                        rot += posed[j].rot * share;
                        pos += posed[j].pos * share;
                        total += share;
                        if j == of {
                            its = share;
                        }
                    }
                }
                if total <= 0.0 {
                    break;
                }
                view.back = (rot * (1.0 / total)).inverse();
                view.carried = its / total;
                let was = view.back * (p - pos / total);
                let moved = was.distance_squared(view.was);
                view.was = was;
                if moved < SAME * SAME {
                    break;
                }
            }
        }
        // Two that find it at the same place read the same there.
        if (0..views.n as usize)
            .any(|v| v != slot && views.of[v].was.distance_squared(view.was) < SAME * SAME)
            && slot + 1 == views.n as usize
        {
            views.n -= 1;
            continue;
        }
        views.of[slot] = view;
    }
    (within, views)
}

/// The measured body at `p`, which was at `from` when the boxes' `views` of it were taken
/// (a few millimeters off at most: the views move with it). Of the boxes that count, the
/// one that finds `p` deepest is right (a chin on a chest is two places).
#[inline]
fn read(boxes: &[TressBox], views: &Views, from: Vec3A, p: Vec3A) -> Option<BodyAt> {
    let mut found: Option<BodyAt> = None;
    for view in &views.of[..views.n as usize] {
        let at = view.was + view.back * (p - from);
        let Some((d, g, _)) = boxes[view.part as usize]
            .at(at)
            .or_else(|| boxes.iter().find_map(|b| b.at(at)))
        else {
            continue;
        };
        if found.is_none_or(|f| d < f.distance) {
            found = Some(BodyAt {
                out: view.back.transpose() * g,
                distance: d,
                part: view.part,
                carried: view.carried,
            });
        }
    }
    found.map(|at| BodyAt {
        out: at.out.try_normalize().unwrap_or(Vec3A::Y),
        ..at
    })
}

/// The measured body at `p` ([`views`], [`read`]), and which of the boxes in `among` `p` is
/// in or within `pad` of.
#[inline]
fn body_at(
    boxes: &[TressBox],
    posed: &[PosedBox],
    among: u16,
    p: Vec3A,
    pad: f32,
    of: usize,
) -> (u16, Option<BodyAt>) {
    let (within, views) = views(boxes, posed, among, p, pad, of);
    (within, read(boxes, &views, p, p))
}

/// A capsule where it is in one substep.
#[derive(Debug, Clone, Copy)]
struct Posed {
    cone: Cone,
    friction: f32,
    body: bool,
    center: Vec3A,
    reach: f32,
}

/// The hair of one person for the solver: on [`Sim::tress`].
#[derive(Debug, Clone, Default)]
pub struct Tress {
    pub config: TressConfig,
    pub verts: Vec<TressVertex>,
    pub strands: Vec<TressStrand>,
    /// The first [`MAX_CAPSULES`] count.
    pub capsules: Vec<TressCapsule>,
    /// The body's measured surface, clothes included (the first [`MAX_BOXES`] count). Hair is
    /// held out of it as out of the capsules.
    pub boxes: Vec<TressBox>,
    /// How wet each particle is, 0 dry to 1 soaked (empty: dry). Wet hair holds its grown
    /// shape and its volume less, gathers into ropes, clings to what it lies on, and the air
    /// moves it less.
    pub wet: Vec<f32>,
    /// Where each strand is cut (empty: none is).
    pub cuts: Vec<TressCut>,
    /// The frame most of the hair rides (the head).
    pub carrier: u16,
    carrier_vel: Vec3,
    /// How wet the hair was on average when it last lay still.
    wet_held: f32,
    carrier_moved: Vec3,
    grid: Grid,
    posed: Vec<Posed>,
    posed_boxes: Vec<PosedBox>,
    /// The boxes where a step starts.
    start_boxes: Vec<PosedBox>,
    box_pads: Vec<f32>,
    /// How far from each capsule a point may start a step and still reach it within it, m.
    pads: Vec<f32>,
    /// How long each strand has lain still, s: from [`SLEEPS_AFTER`] on it sleeps.
    still: Vec<f32>,
    /// Every frame against the head, and the head's turn, when the body last moved.
    held_at: Vec<(Vec3, Quat)>,
}

impl Default for TressStrand {
    fn default() -> Self {
        Self {
            first: 0,
            count: 0,
            frame: 0,
            seed: 0.0,
        }
    }
}

#[inline]
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// `x.floor()` for |x| < 2³¹ without the call the baseline x86-64 target makes of it.
#[inline]
fn floor(x: f32) -> f32 {
    let t = x as i32 as f32;
    if t > x { t - 1.0 } else { t }
}

#[inline]
fn floor3(v: Vec3) -> Vec3 {
    Vec3::new(floor(v.x), floor(v.y), floor(v.z))
}

#[inline]
fn fract(v: Vec3) -> Vec3 {
    v - floor3(v)
}

fn hash31(p: Vec3) -> f32 {
    let mut q = fract(p * Vec3::new(0.1031, 0.1030, 0.0973));
    q += Vec3::splat(q.dot(Vec3::new(q.y, q.x, q.z) + 33.33));
    let v = (q.x + q.y) * q.z;
    (v - floor(v)) * 2.0 - 1.0
}

/// Value noise with its gradient (quintic, C2).
fn noise_grad(x: Vec3) -> (f32, Vec3) {
    let i = floor3(x);
    let f = x - i;
    let u = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    let du = 30.0 * f * f * (f * (f - 2.0) + 1.0);
    let at = |x: f32, y: f32, z: f32| hash31(i + Vec3::new(x, y, z));
    let (a, b, c, d) = (
        at(0.0, 0.0, 0.0),
        at(1.0, 0.0, 0.0),
        at(0.0, 1.0, 0.0),
        at(1.0, 1.0, 0.0),
    );
    let (e, f1, g, h) = (
        at(0.0, 0.0, 1.0),
        at(1.0, 0.0, 1.0),
        at(0.0, 1.0, 1.0),
        at(1.0, 1.0, 1.0),
    );
    let (k0, k1, k2, k3) = (a, b - a, c - a, e - a);
    let (k4, k5, k6) = (a - b - c + d, a - c - e + g, a - b - e + f1);
    let k7 = -a + b + c - d + e - f1 - g + h;
    let val = k0
        + k1 * u.x
        + k2 * u.y
        + k3 * u.z
        + k4 * u.x * u.y
        + k5 * u.y * u.z
        + k6 * u.z * u.x
        + k7 * u.x * u.y * u.z;
    let grad = Vec3::new(
        du.x * (k1 + k4 * u.y + k6 * u.z + k7 * u.y * u.z),
        du.y * (k2 + k5 * u.z + k4 * u.x + k7 * u.z * u.x),
        du.z * (k3 + k6 * u.x + k5 * u.y + k7 * u.x * u.y),
    );
    (val, grad)
}

/// Divergence-free turbulence: the curl of three noise potentials, two octaves.
fn curl_noise(p: Vec3) -> Vec3 {
    let curl = |p: Vec3, o2: Vec3, o3: Vec3| {
        let (_, g1) = noise_grad(p);
        let (_, g2) = noise_grad(p + o2);
        let (_, g3) = noise_grad(p + o3);
        Vec3::new(g3.y - g2.z, g1.z - g3.x, g2.x - g1.y)
    };
    let c1 = curl(
        p,
        Vec3::new(31.416, 17.321, 59.265),
        Vec3::new(73.112, 41.778, 19.634),
    );
    let c2 = curl(
        p * 2.13 + Vec3::new(11.7, -5.3, 23.1),
        Vec3::new(27.1, 43.9, 13.7),
        Vec3::new(53.3, 7.1, 61.9),
    );
    (c1 * 0.72 + c2 * 0.28) * 0.32
}

/// A round cone (a capsule with radius `r1` at `a` and `r2` at `b`) readied for many
/// distance queries.
#[derive(Debug, Clone, Copy)]
struct Cone {
    a: Vec3A,
    b: Vec3A,
    r1: f32,
    r2: f32,
    ba: Vec3A,
    axis: Vec3A,
    l2: f32,
    il2: f32,
    rr: f32,
    a2: f32,
    /// `sign(rr) rr²`.
    krr: f32,
    /// `sqrt(a2 / l2)` and `rr / l`: the side's normal is that much across and along.
    across: f32,
    along: f32,
    /// One sphere stands for it: no length, or one end's sphere holds the other.
    ball: Option<(Vec3A, f32)>,
}

impl Cone {
    fn new(a: Vec3A, b: Vec3A, r1: f32, r2: f32) -> Self {
        let ba = b - a;
        let l2 = ba.length_squared();
        let rr = r1 - r2;
        let a2 = l2 - rr * rr;
        let mut cone = Self {
            a,
            b,
            r1,
            r2,
            ba,
            axis: Vec3A::Y,
            l2,
            il2: 0.0,
            rr,
            a2,
            krr: rr * rr.abs(),
            across: 1.0,
            along: 0.0,
            ball: None,
        };
        if l2 < 1e-10 {
            cone.ball = Some((a, r1.max(r2)));
        } else if a2 <= 1e-8 {
            cone.ball = Some((if r1 >= r2 { a } else { b }, r1.max(r2)));
        } else {
            cone.il2 = 1.0 / l2;
            let inv_l = cone.il2.sqrt();
            cone.axis = ba * inv_l;
            cone.across = (a2 * cone.il2).sqrt();
            cone.along = rr * inv_l;
        }
        cone
    }

    /// The outward normal at `p` and its signed distance. Exact.
    #[inline]
    fn at(&self, p: Vec3A) -> (Vec3A, f32) {
        let ball = |c: Vec3A, r: f32, fallback: Vec3A| {
            let d = p - c;
            let len = d.length();
            (if len > 1e-6 { d / len } else { fallback }, len - r)
        };
        if let Some((c, r)) = self.ball {
            return ball(c, r, Vec3A::Y);
        }
        let pa = p - self.a;
        let y = pa.dot(self.ba);
        let z = y - self.l2;
        let x_vec = pa * self.l2 - self.ba * y;
        let x2 = x_vec.length_squared();
        let k = self.krr * x2;
        let al2 = self.a2 * self.l2;
        if z * z.abs() * al2 > k {
            return ball(self.b, self.r2, self.axis);
        }
        if y * y.abs() * al2 < k {
            return ball(self.a, self.r1, -self.axis);
        }
        let dist = ((x2 * self.a2 * self.il2).sqrt() + y * self.rr) * self.il2 - self.r1;
        let radial = if x2 < 1e-10 {
            self.axis
                .cross(Vec3A::Z)
                .try_normalize()
                .unwrap_or(Vec3A::X)
        } else {
            x_vec / x2.sqrt()
        };
        let n = (radial * self.across + self.axis * self.along)
            .try_normalize()
            .unwrap_or(radial);
        (n, dist)
    }
}

/// Signed distance from `p` to a round cone (a capsule with radius `r1` at `a` and `r2` at
/// `b`), and the outward normal there. Exact.
pub fn tapered_capsule(p: Vec3, a: Vec3, b: Vec3, r1: f32, r2: f32) -> (Vec3, f32) {
    let (n, d) = Cone::new(a.into(), b.into(), r1, r2).at(p.into());
    (n.into(), d)
}

/// `e` turned `part` of the way of the rotation that takes direction `a` to direction `b`:
/// about the same axis, by the angle a normalized blend of the two rotations gives (within
/// 4% of `part` of the angle up to 150 degrees, and no trigonometry).
#[inline]
fn turned(e: Vec3A, a: Vec3A, b: Vec3A, part: f32) -> Vec3A {
    let c = a.dot(b);
    // The whole turn as a quaternion (xyz, w), then its blend with no turn.
    let (xyz, w) = if c > -0.9999 {
        let half = (0.5 + 0.5 * c).sqrt();
        (a.cross(b) * (0.5 / half), half)
    } else {
        (a.any_orthonormal_vector(), 0.0)
    };
    let (xyz, w) = (xyz * part, 1.0 - part + part * w);
    let inv = (xyz.length_squared() + w * w).sqrt().recip();
    let (xyz, w) = (xyz * inv, w * inv);
    let t = xyz.cross(e) * 2.0;
    e + t * w + xyz.cross(t)
}

/// Hair's density and momentum around the head, for its pressure against itself and the pull
/// toward its mean velocity.
#[derive(Debug, Clone, Default)]
struct Grid {
    min: Vec3,
    cell: f32,
    dims: [usize; 3],
    /// What every point adds to the cells around it.
    weight: f32,
    density: Vec<f32>,
    momentum: Vec<Vec3>,
}

impl Grid {
    /// `pos` are the points in the head's frame (the cells are a lattice fixed to the head, so
    /// what the hair feels from itself doesn't change with where the person stands or faces,
    /// and still hair rests in one place); `vel` their velocities in the world.
    fn build(
        &mut self,
        pos: impl Iterator<Item = Vec3> + Clone,
        vel: &[Vec3],
        weight: f32,
        cell: f32,
    ) {
        let (mut lo, mut hi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
        for p in pos.clone() {
            if p.is_finite() {
                lo = lo.min(p);
                hi = hi.max(p);
            }
        }
        if !(lo.is_finite() && hi.is_finite()) {
            self.dims = [0; 3];
            return;
        }
        let extent = (hi - lo).max_element();
        self.cell = cell.max(extent / (GRID_MAX - 6) as f32);
        self.weight = weight;
        self.min = ((lo / self.cell).floor() - Vec3::splat(2.0)) * self.cell;
        let dims = ((hi - self.min) / self.cell).ceil() + Vec3::splat(3.0);
        self.dims = [
            (dims.x as usize).min(GRID_MAX),
            (dims.y as usize).min(GRID_MAX),
            (dims.z as usize).min(GRID_MAX),
        ];
        let cells = self.dims[0] * self.dims[1] * self.dims[2];
        self.density.clear();
        self.density.resize(cells, 0.0);
        self.momentum.clear();
        self.momentum.resize(cells, Vec3::ZERO);
        for (p, v) in pos.zip(vel) {
            let Some((c, f)) = self.cell_of(p) else {
                continue;
            };
            let g = Vec3::ONE - f;
            for (dz, wz) in [(0, g.z), (1, f.z)] {
                for (dy, wy) in [(0, g.y), (1, f.y)] {
                    for (dx, wx) in [(0, g.x), (1, f.x)] {
                        let w = weight * wx * wy * wz;
                        let i = self.index(c[0] + dx, c[1] + dy, c[2] + dz);
                        self.density[i] += w;
                        self.momentum[i] += *v * w;
                    }
                }
            }
        }
    }

    #[inline]
    fn index(&self, x: usize, y: usize, z: usize) -> usize {
        (z * self.dims[1] + y) * self.dims[0] + x
    }

    /// The cell `p` is in and where in it (0..1), when all eight corners around it exist.
    #[inline]
    fn cell_of(&self, p: Vec3) -> Option<([usize; 3], Vec3)> {
        if self.dims[0] < 2 || self.dims[1] < 2 || self.dims[2] < 2 {
            return None;
        }
        let rel = (p - self.min) / self.cell - Vec3::splat(0.5);
        let max = Vec3::new(
            self.dims[0] as f32 - 1.001,
            self.dims[1] as f32 - 1.001,
            self.dims[2] as f32 - 1.001,
        );
        if !(rel.cmpge(Vec3::ZERO).all() && rel.cmple(max).all()) {
            return None;
        }
        let c = rel.floor();
        Some(([c.x as usize, c.y as usize, c.z as usize], rel - c))
    }

    /// Density, its gradient (per meter) and the mean velocity of the hair around the point
    /// at `p` moving at `own`, without what that point itself added to the grid: its own
    /// share has a gradient wherever the point isn't on a cell corner, and left in, every
    /// point pushes itself about (with few strands a cell, hair in still air never rests).
    fn sample(&self, p: Vec3, own: Vec3) -> (f32, Vec3, Vec3) {
        let Some((c, f)) = self.cell_of(p) else {
            return (0.0, Vec3::ZERO, Vec3::ZERO);
        };
        let g = Vec3::ONE - f;
        // The point's own share where it is: the sum of its squared corner weights.
        let sq = g * g + f * f;
        let own_density = self.weight * sq.x * sq.y * sq.z;
        let own_grad = Vec3::new(
            (2.0 * f.x - 1.0) * sq.y * sq.z,
            (2.0 * f.y - 1.0) * sq.z * sq.x,
            (2.0 * f.z - 1.0) * sq.x * sq.y,
        ) * (self.weight / self.cell);
        let i = |dx: usize, dy: usize, dz: usize| self.index(c[0] + dx, c[1] + dy, c[2] + dz);
        let ids = [
            i(0, 0, 0),
            i(1, 0, 0),
            i(0, 1, 0),
            i(1, 1, 0),
            i(0, 0, 1),
            i(1, 0, 1),
            i(0, 1, 1),
            i(1, 1, 1),
        ];
        let d = ids.map(|k| self.density[k]);
        let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
        let density = lerp(
            lerp(lerp(d[0], d[1], f.x), lerp(d[2], d[3], f.x), f.y),
            lerp(lerp(d[4], d[5], f.x), lerp(d[6], d[7], f.x), f.y),
            f.z,
        );
        let density = (density - own_density).max(0.0);
        let inv = 1.0 / self.cell;
        let grad = Vec3::new(
            (d[1] - d[0]) * g.y * g.z
                + (d[3] - d[2]) * f.y * g.z
                + (d[5] - d[4]) * g.y * f.z
                + (d[7] - d[6]) * f.y * f.z,
            (d[2] - d[0]) * g.x * g.z
                + (d[3] - d[1]) * f.x * g.z
                + (d[6] - d[4]) * g.x * f.z
                + (d[7] - d[5]) * f.x * f.z,
            (d[4] - d[0]) * g.x * g.y
                + (d[5] - d[1]) * f.x * g.y
                + (d[6] - d[2]) * g.x * f.y
                + (d[7] - d[3]) * f.x * f.y,
        ) * inv
            - own_grad;
        let mut mean = Vec3::ZERO;
        if density > 0.25 {
            let m = ids.map(|k| self.momentum[k]);
            let momentum = m[0]
                .lerp(m[1], f.x)
                .lerp(m[2].lerp(m[3], f.x), f.y)
                .lerp(m[4].lerp(m[5], f.x).lerp(m[6].lerp(m[7], f.x), f.y), f.z);
            mean = (momentum - own * own_density) / density.max(0.5);
        }
        (density, grad, mean)
    }
}

/// What every strand reads during one step.
struct Step<'a> {
    cfg: TressConfig,
    verts: &'a [TressVertex],
    frames: &'a [crate::solver::Frame],
    /// The capsules in every substep, `caps` of them each.
    posed: &'a [Posed],
    caps: usize,
    pads: &'a [f32],
    /// The body's boxes, and where they are in every substep.
    boxes: &'a [TressBox],
    posed_boxes: &'a [PosedBox],
    start_boxes: &'a [PosedBox],
    box_pads: &'a [f32],
    wet: &'a [f32],
    grid: &'a Grid,
    ground: &'a [[f32; 4]],
    substeps: u32,
    dt: f32,
    time: f32,
    gravity: Vec3,
    /// The head, its velocity this step, and how much of its jolt the hair follows rigidly.
    carrier: crate::solver::Frame,
    carrier_vel: Vec3,
    carried: Vec3,
    wind_dir: Vec3,
    wind_strength: f32,
    /// Extra damping, 1/s (a body settling).
    settle: f32,
}

impl Tress {
    /// Whether the hair can be stepped as `sim`'s particles.
    pub fn fits(&self, sim: &Sim) -> bool {
        self.verts.len() == sim.pos.len()
            && self.strands.iter().all(|s| {
                (s.first + s.count) as usize <= sim.pos.len()
                    && (s.frame as usize) < sim.frames.len()
                    && s.count as usize <= MAX_VERTS
            })
            && self
                .capsules
                .iter()
                .all(|c| (c.frame as usize) < sim.frames.len())
            && self
                .boxes
                .iter()
                .all(|b| (b.frame as usize) < sim.frames.len() && b.whole())
            && (self.carrier as usize) < sim.frames.len()
    }

    /// The measured body at `p` with `frames` where the step left them: the way out, the
    /// signed distance, and the share of it the frame the hair rides (the head) carries.
    /// `None` outside every box.
    pub fn surface_at(&self, frames: &[crate::solver::Frame], p: Vec3) -> Option<(Vec3, f32, f32)> {
        let boxes = &self.boxes[..self.boxes.len().min(MAX_BOXES)];
        let mut posed = [PosedBox::default(); MAX_BOXES];
        for (at, b) in posed.iter_mut().zip(boxes) {
            let f = frames.get(b.frame as usize)?;
            (at.pos, at.rot) = (f.to_pos.into(), Mat3A::from_quat(f.to_rot));
        }
        let head = boxes.iter().position(|b| b.frame == self.carrier);
        body_at(
            boxes,
            &posed,
            u16::MAX,
            p.into(),
            0.0,
            head.unwrap_or(usize::MAX),
        )
        .1
        .map(|at| (at.out.into(), at.distance, at.carried))
    }

    /// Cut strand `strand` in the segment into its particle `at`, `part` of the way along it:
    /// the particles from there on come off as a piece that falls free. Whether anything was
    /// cut (not the root, not past an earlier cut, not a piece already off).
    pub fn cut(&mut self, strand: usize, at: usize, part: f32) -> bool {
        let Some(count) = self.strands.get(strand).map(|s| s.count as usize) else {
            return false;
        };
        if self.cuts.len() != self.strands.len() {
            self.cuts.clear();
            self.cuts.resize(self.strands.len(), TressCut::default());
        }
        let was = self.cuts[strand];
        let whole = if was.at == 0 { count } else { was.at as usize };
        if at == 0 || at >= whole {
            return false;
        }
        self.cuts[strand] = TressCut {
            at: at as u16,
            end: whole as u16,
            part: part.clamp(0.0, 1.0),
            shed: 0.0,
        };
        if let Some(still) = self.still.get_mut(strand) {
            *still = 0.0;
        }
        true
    }

    /// Strand `strand` was moved from outside: it no longer sleeps.
    pub fn wake(&mut self, strand: usize) {
        if let Some(still) = self.still.get_mut(strand) {
            *still = 0.0;
        }
    }

    /// The share of strand `strand`'s length a cut left (1: whole).
    pub fn left(&self, strand: usize) -> f32 {
        let (Some(cut), Some(s)) = (self.cuts.get(strand), self.strands.get(strand)) else {
            return 1.0;
        };
        if cut.at == 0 {
            return 1.0;
        }
        let v = &self.verts[s.first as usize..(s.first + s.count) as usize];
        let whole: f32 = v[1..].iter().map(|v| v.rest_len).sum();
        let kept: f32 = v[1..cut.at as usize]
            .iter()
            .map(|v| v.rest_len)
            .sum::<f32>()
            + v[cut.at as usize].rest_len * cut.part;
        (kept / whole.max(1e-6)).clamp(0.0, 1.0)
    }

    /// Forget the head's last velocity (after a teleport, or putting the hair at rest).
    pub fn calm(&mut self) {
        self.carrier_vel = Vec3::ZERO;
        self.carrier_moved = Vec3::ZERO;
        self.still.clear();
    }

    /// How many strands sleep.
    pub fn asleep(&self) -> usize {
        self.still.iter().filter(|s| **s >= SLEEPS_AFTER).count()
    }

    /// Whether the body or the air moved since the hair last lay still: the frames against
    /// the head, the head's turn (where down is), and the air past it.
    fn disturbed(&mut self, sim: &Sim, air: f32) -> bool {
        let carrier = sim.frames[self.carrier as usize];
        let to_head = carrier.to_rot.conjugate();
        let now = |k: usize, f: &crate::solver::Frame| {
            if k == self.carrier as usize {
                (Vec3::ZERO, carrier.to_rot)
            } else {
                (to_head * (f.to_pos - carrier.to_pos), to_head * f.to_rot)
            }
        };
        let moved = air > STILL_AIR
            || self.held_at.len() != sim.frames.len()
            || sim.frames.iter().enumerate().any(|(k, f)| {
                let ((pos, rot), (was_pos, was_rot)) = (now(k, f), self.held_at[k]);
                pos.distance_squared(was_pos) > STILL_BONE * STILL_BONE
                    || rot.dot(was_rot).abs() < STILL_TURN
            });
        if moved {
            self.held_at.clear();
            let held: Vec<_> = sim
                .frames
                .iter()
                .enumerate()
                .map(|(k, f)| now(k, f))
                .collect();
            self.held_at = held;
        }
        moved
    }

    /// Advance `sim`'s particles by `dt`: its frames are the bones from the step's start to
    /// its end, its wind the air at the head (m/s).
    pub fn step(&mut self, sim: &mut Sim, dt: f32) {
        if !self.fits(sim) || dt <= 0.0 {
            return;
        }
        let cfg = self.config;
        let per = cfg.substeps.max(1);
        let substeps = ((per as f32 * dt * 60.0 - 1e-3).ceil() as u32).clamp(1, per * 4);
        // The head through the air: a run is wind in the hair, and what the hair's own motion
        // is measured against.
        let carrier = sim.frames[self.carrier as usize];
        let moved = (carrier.to_pos - carrier.from_pos) / dt;
        let jolt = (moved - self.carrier_moved).length() / dt;
        self.carrier_moved = moved;
        let follow = FOLLOWED * smoothstep(60.0, 400.0, jolt);
        // Its velocity over the last tenth of a second or so: a single step's jerk isn't a
        // speed the hair is carried at (measured against it, the speed limit flung the hair
        // ahead of a head moved 30 cm in one step).
        self.carrier_vel += (moved - self.carrier_vel) * (dt / CARRIED_OVER).min(1.0);
        let carrier_vel = self.carrier_vel;
        let wind = sim.params.wind - carrier_vel;
        let speed = wind.length();
        let (wind_dir, wind_strength) = if speed > 1e-4 {
            (wind / speed, (speed * cfg.wind_feel).min(6.0))
        } else {
            (Vec3::X, 0.0)
        };
        // The grid's cells hold a few strands each, as the source's do with its 1792 guides:
        // fewer strands take larger cells, and each point weighs what makes the densities
        // the source's constants are for.
        let cell = (CELL * (SOURCE_STRANDS / self.strands.len().max(1) as f32).sqrt())
            .clamp(CELL, CELL * 3.0);
        let weight = SOURCE_POINTS / self.verts.len().max(1) as f32 * (CELL / cell).powi(3);
        let to_head = carrier.from_rot.conjugate();
        self.grid.build(
            sim.pos.iter().map(|p| to_head * (*p - carrier.from_pos)),
            &sim.vel,
            weight,
            cell,
        );
        // Hair that lies still sleeps until the body or the air moves.
        if self.still.len() != self.strands.len() {
            self.still.clear();
            self.still.resize(self.strands.len(), 0.0);
            self.held_at.clear();
        }
        // (Hair that gets wet or dries hangs differently: it wakes for every tenth of the way.)
        let wet = self.wet.iter().sum::<f32>() / self.wet.len().max(1) as f32;
        let wetter = (wet - self.wet_held).abs() > 0.1;
        if wetter {
            self.wet_held = wet;
        }
        if self.disturbed(sim, speed) || wetter {
            self.still.fill(0.0);
        }
        // Pieces cut off fall for a while, awake, and are gone.
        if self.cuts.len() != self.strands.len() {
            self.cuts.clear();
        }
        for (cut, still) in self.cuts.iter_mut().zip(&mut self.still) {
            if cut.at != 0 && cut.shed < 1.0 {
                cut.shed = (cut.shed + dt / SHED_SECS).min(1.0);
                *still = 0.0;
            }
        }
        let caps = self.capsules.len().min(MAX_CAPSULES);
        self.posed.clear();
        for s in 0..substeps {
            let frac = (s + 1) as f32 / substeps as f32;
            for c in &self.capsules[..caps] {
                let frame = sim.frames[c.frame as usize];
                let (pos, rot) = frame.pose(frac);
                let (a, b) = (pos + rot * c.a, pos + rot * c.b);
                self.posed.push(Posed {
                    cone: Cone::new(a.into(), b.into(), c.ra, c.rb),
                    friction: c.friction,
                    body: c.body,
                    center: ((a + b) * 0.5).into(),
                    reach: a.distance(b) * 0.5 + c.ra.max(c.rb),
                });
            }
        }
        // A point moves against the head's velocity no faster than the speed limit, and a
        // capsule as its bone does: how near the two must start a step to meet within it.
        let h = dt / substeps as f32;
        let carried = (carrier.to_pos - carrier.from_pos) * follow;
        let slack = 0.045 + MAX_SPEED * dt + carrier_vel.length() * h + carried.length();
        self.pads.clear();
        for k in 0..caps {
            let first = self.posed[k].center;
            let travel = (1..substeps as usize)
                .map(|s| {
                    (self.posed[s * caps + k].center
                        - first
                        - Vec3A::from(carrier_vel) * (s as f32 * h))
                        .length()
                })
                .fold(0.0, f32::max);
            self.pads.push(slack + travel);
        }
        let boxes = &self.boxes[..self.boxes.len().min(MAX_BOXES)];
        self.posed_boxes.clear();
        self.start_boxes.clear();
        for s in 0..=substeps {
            let frac = s as f32 / substeps as f32;
            for b in boxes {
                let (pos, rot) = sim.frames[b.frame as usize].pose(frac);
                let half = Vec3::new(
                    (b.size[0] - 1) as f32,
                    (b.size[1] - 1) as f32,
                    (b.size[2] - 1) as f32,
                ) * (b.cell * 0.5);
                let posed = PosedBox {
                    pos: pos.into(),
                    rot: Mat3A::from_quat(rot),
                    center: (pos + rot * (b.min + half)).into(),
                    reach: half.length(),
                };
                if s == 0 {
                    self.start_boxes.push(posed);
                } else {
                    self.posed_boxes.push(posed);
                }
            }
        }
        self.box_pads.clear();
        for k in 0..boxes.len() {
            let first = self.posed_boxes[k].center;
            let travel = (1..substeps as usize)
                .map(|s| {
                    (self.posed_boxes[s * boxes.len() + k].center
                        - first
                        - Vec3A::from(carrier_vel) * (s as f32 * h))
                        .length()
                })
                .fold(0.0, f32::max);
            self.box_pads.push(slack + travel);
        }
        let step = Step {
            cfg,
            verts: &self.verts,
            frames: &sim.frames,
            posed: &self.posed,
            caps,
            pads: &self.pads,
            boxes,
            posed_boxes: &self.posed_boxes,
            start_boxes: &self.start_boxes,
            box_pads: &self.box_pads,
            wet: if self.wet.len() == self.verts.len() {
                &self.wet
            } else {
                &[]
            },
            grid: &self.grid,
            ground: &sim.ground,
            substeps,
            dt,
            time: sim.params.time,
            gravity: sim.params.gravity,
            carrier,
            carrier_vel,
            carried: carried / substeps as f32,
            wind_dir,
            wind_strength,
            settle: sim.params.damping.max(0.0),
        };
        // Strands share nothing but what they read: runs of whole strands side by side.
        let threads = leaf_pool().thread_num().max(1);
        let per_run = (self.strands.len().div_ceil(threads)).max(16);
        let mut runs = Vec::new();
        let (mut pos, mut vel) = (&mut sim.pos[..], &mut sim.vel[..]);
        let mut at = 0usize;
        let cuts = &self.cuts[..];
        for (run, (strands, still)) in self
            .strands
            .chunks(per_run)
            .zip(self.still.chunks_mut(per_run))
            .enumerate()
        {
            let cuts = cuts.get(run * per_run..).unwrap_or(&[]);
            let end = strands
                .iter()
                .map(|s| (s.first + s.count) as usize)
                .max()
                .unwrap_or(at);
            let start = strands.iter().map(|s| s.first as usize).min().unwrap_or(at);
            if start < at || end < start {
                // Strands out of order: nothing to hand out safely.
                return;
            }
            let (_, p) = std::mem::take(&mut pos).split_at_mut(start - at);
            let (_, v) = std::mem::take(&mut vel).split_at_mut(start - at);
            let (p, rest_p) = p.split_at_mut(end - start);
            let (v, rest_v) = v.split_at_mut(end - start);
            (pos, vel) = (rest_p, rest_v);
            at = end;
            runs.push((strands, still, cuts, start, p, v));
        }
        let step = &step;
        if runs.len() > 1 {
            leaf_pool().scope(|scope| {
                for (strands, still, cuts, base, p, v) in runs {
                    scope.spawn(async move {
                        for (k, (s, still)) in strands.iter().zip(still).enumerate() {
                            let cut = cuts.get(k).copied().unwrap_or_default();
                            step.strand(s, cut, still, base, p, v);
                        }
                    });
                }
            });
        } else {
            for (strands, still, cuts, base, p, v) in runs {
                for (k, (s, still)) in strands.iter().zip(still).enumerate() {
                    let cut = cuts.get(k).copied().unwrap_or_default();
                    step.strand(s, cut, still, base, p, v);
                }
            }
        }
    }
}

impl Step<'_> {
    /// Which of the capsules in `among` are within `pad` (or each its own of `pads`) of
    /// touching `p`.
    #[inline]
    fn near(caps: &[Posed], among: u32, p: Vec3A, pad: f32, pads: Option<&[f32]>) -> u32 {
        let (mut left, mut near) = (among, 0u32);
        while left != 0 {
            let k = left.trailing_zeros() as usize;
            left &= left - 1;
            let c = &caps[k];
            let reach = c.reach + pad + pads.map_or(0.0, |pads| pads[k]);
            if p.distance_squared(c.center) < reach * reach {
                near |= 1 << k;
            }
        }
        near
    }

    /// The body's measured surface at `p` in substep `s`, from the boxes in `among`
    /// ([`body_at`]): the way out and the signed distance. `None` outside them all.
    #[inline]
    fn surface(&self, s: usize, among: u16, p: Vec3A) -> Option<(Vec3A, f32)> {
        let posed = &self.posed_boxes[s * self.boxes.len()..(s + 1) * self.boxes.len()];
        body_at(self.boxes, posed, among, p, 0.0, usize::MAX)
            .1
            .map(|at| (at.out, at.distance))
    }

    /// The boxes `p` could be in within the step.
    #[inline]
    fn near_boxes(&self, p: Vec3A) -> u16 {
        let mut near = 0u16;
        for (k, at) in self.posed_boxes[..self.boxes.len()].iter().enumerate() {
            let reach = at.reach + self.box_pads[k];
            if p.distance_squared(at.center) < reach * reach
                && self.boxes[k].holds(at.rot.transpose() * (p - at.pos), self.box_pads[k])
            {
                near |= 1 << k;
            }
        }
        near
    }

    /// Margin kept from a capsule by a point grown `sunk` deep in its kind.
    #[inline]
    fn margin(base: f32, sunk: [f32; 2], body: bool) -> f32 {
        base - sunk[body as usize]
    }

    /// The capsules of `among` that `p` could touch in a substep's constraint passes, with
    /// how far off each is now.
    fn close(&self, caps: &[Posed], among: u32, p: Vec3A, base: f32, sunk: [f32; 2]) -> Close {
        let mut close = Close {
            at: p,
            ..Close::default()
        };
        let mut left = Self::near(caps, among, p, REACHED + FILLET + base, None);
        while left != 0 {
            let k = left.trailing_zeros() as usize;
            left &= left - 1;
            let c = &caps[k];
            let sd = c.cone.at(p).1;
            if sd - Self::margin(base, sunk, c.body) > REACHED + FILLET {
                continue;
            }
            let n = close.n as usize;
            if n < close.cap.len() {
                (close.cap[n], close.sd[n]) = (k as u8, sd);
                close.n += 1;
            } else if let Some(worst) = (0..n).max_by(|a, b| close.sd[*a].total_cmp(&close.sd[*b]))
                && sd < close.sd[worst]
            {
                (close.cap[worst], close.sd[worst]) = (k as u8, sd);
            }
        }
        close
    }

    /// Push `p` out of the capsules `close` to it: the
    /// body's blended smooth first (a 5 mm fillet in the creases of neck and shoulders), then
    /// every capsule exactly. Friction holds the point where it is pressed on, and acts on
    /// `prev`.
    #[allow(clippy::too_many_arguments)]
    fn resolve(
        &self,
        caps: &[Posed],
        close: &Close,
        mut p: Vec3A,
        mut prev: Vec3A,
        base: f32,
        sunk: [f32; 2],
        smooth_body: bool,
        wet: f32,
        grip: &mut Grip,
    ) -> (Vec3A, Vec3A) {
        // Wet hair clings to what it lies on.
        let cling = WET_CLINGS * wet;
        // A distance changes no more than the point moves: what was further off than the
        // point has come since is still clear.
        let moved = p.distance(close.at);
        let near = &close.cap[..close.n as usize];
        if smooth_body {
            let margin = Self::margin(base, sunk, true);
            let mut found: Option<(f32, Vec3A, f32)> = None;
            for (k, sd) in near.iter().zip(close.sd) {
                let c = &caps[*k as usize];
                if !c.body || sd - moved > margin + FILLET {
                    continue;
                }
                let (n, sd) = c.cone.at(p);
                let d = sd - margin;
                let fric = self.cfg.friction.max(c.friction) + cling;
                found = Some(match found {
                    None => (d, n, fric),
                    Some((sd0, n0, f0)) => {
                        let h = (FILLET - (sd0 - d).abs()).max(0.0) / FILLET;
                        let blend = sd0.min(d) - 0.25 * FILLET * h * h;
                        let smaller = d < sd0;
                        let (n_min, n_other) = if smaller { (n, n0) } else { (n0, n) };
                        let n = n_min
                            .lerp(n_other, 0.5 * h)
                            .try_normalize()
                            .unwrap_or(n_min);
                        (blend, n, if smaller { fric } else { f0 })
                    }
                });
            }
            if let Some((d, n, fric)) = found
                && d < 0.0
            {
                p -= n * d;
                let dp = p - prev;
                let dn = dp.dot(n);
                let fric = fric.clamp(0.0, 0.94);
                prev = p - (dp - n * dn) * (1.0 - fric * 0.55) - n * dn.max(0.0) * 0.08;
            }
        }
        let moved = p.distance(close.at);
        for (k, sd) in near.iter().zip(close.sd) {
            let c = &caps[*k as usize];
            let margin = Self::margin(base, sunk, c.body);
            if sd - moved > margin {
                continue;
            }
            let (n, sd) = c.cone.at(p);
            if sd < margin {
                let fric = (self.cfg.friction.max(c.friction) + cling).clamp(0.0, 0.94);
                p += n * (margin - sd);
                let dp = p - prev;
                let dn = dp.dot(n);
                prev = p - (dp - n * dn) * (1.0 - fric * 0.50) - n * dn.max(0.0) * 0.05;
            }
        }
        // The body's measured surface, last: it is what shows.
        let margin = Self::margin(base, sunk, true);
        if close.surface - p.distance(close.at) <= margin
            && let Some(BodyAt {
                out: n,
                distance: sd,
                part,
                ..
            }) = read(self.boxes, &close.views, close.at, p)
            && sd < margin
        {
            let fric = (self.cfg.friction + cling).clamp(0.0, 0.94);
            let push = margin - sd;
            p += n * push;
            *grip = Grip {
                pressed: grip.pressed + push,
                n,
                part,
            };
            let dp = p - prev;
            let dn = dp.dot(n);
            // Friction takes of the point's slide no more than what presses it on gives
            // (Coulomb's, in positions). Taking its share of any slide however lightly the
            // hair lay there, a lock's end on a back stayed where it was like a rope's end on
            // a table: the lock's slack went over to the other side as the head turned, and
            // the end whipped out after it.
            let slid = dp - n * dn;
            let far = slid.length();
            // (Wet hair clings to what it lies on without being pressed there.)
            let lost = (far * fric * 0.50).min(RUBS * push + cling * (push + WET_HOLDS));
            let kept = if far > 1e-9 { 1.0 - lost / far } else { 1.0 };
            prev = p - slid * kept - n * dn.max(0.0) * 0.05;
        }
        (p, prev)
    }

    /// `p` at exactly `len` from `parent` and outside every capsule `close` to it: onto the
    /// sphere around the parent, out of what it is in, then along that capsule's tangent
    /// plane back to the sphere.
    #[allow(clippy::too_many_arguments)]
    fn exact(
        &self,
        caps: &[Posed],
        close: &Close,
        mut p: Vec3A,
        parent: Vec3A,
        len: f32,
        base: f32,
        sunk: [f32; 2],
        fallback: Vec3A,
    ) -> Vec3A {
        let onto = |p: Vec3A| {
            let delta = p - parent;
            let dist = delta.length();
            if dist > 1e-7 {
                parent + delta * (len / dist)
            } else {
                parent + fallback * len
            }
        };
        p = onto(p);
        let near = &close.cap[..close.n as usize];
        // What the point is within a centimeter of: it moves no further in these passes.
        let moved = p.distance(close.at) + 0.01;
        let mut left = 0u32;
        for (i, sd) in close.sd[..near.len()].iter().enumerate() {
            let body = caps[near[i] as usize].body;
            if sd - moved <= Self::margin(base, sunk, body) {
                left |= 1 << i;
            }
        }
        let on_body = Self::margin(base, sunk, true);
        // (The measured surface is asked again only as near as the point has really come.)
        if left == 0 && close.surface - moved > on_body {
            return p;
        }
        // Out of a shape by `depth` along `n`, then along its tangent plane back to the sphere.
        let slide = |mut p: Vec3A, n: Vec3A, depth: f32| {
            p += n * depth;
            let rel = p - parent;
            let h = rel.dot(n);
            if h.abs() < len * 0.999 {
                let tang = rel - n * h;
                let along = tang.try_normalize().unwrap_or_else(|| {
                    (fallback - n * fallback.dot(n))
                        .try_normalize()
                        .or_else(|| n.cross(Vec3A::Y).try_normalize())
                        .unwrap_or(Vec3A::X)
                });
                p = parent + n * h + along * (len * len - h * h).max(0.0).sqrt();
            }
            p
        };
        let surface = |p: Vec3A| {
            if close.surface - p.distance(close.at) > on_body {
                return None;
            }
            read(self.boxes, &close.views, close.at, p)
                .map(|at| (at.out, at.distance))
                .filter(|(_, sd)| *sd < on_body)
        };
        for pass in 0..5 {
            if pass > 0 {
                p = onto(p);
            }
            let mut hit = false;
            let mut some = left;
            while some != 0 {
                let c = &caps[near[some.trailing_zeros() as usize] as usize];
                some &= some - 1;
                let (n, sd) = c.cone.at(p);
                let margin = Self::margin(base, sunk, c.body);
                if sd >= margin {
                    continue;
                }
                hit = true;
                p = slide(p, n, margin - sd);
            }
            if let Some((n, sd)) = surface(p) {
                hit = true;
                // (Straight out, and back onto the sphere in the next pass. Slid along the
                // surface as along a capsule, a point where the strand is folded over a
                // collar loads the strand's bends at every snap: the lock below swings for
                // good.)
                p += n * (on_body - sd);
            }
            if !hit {
                return p;
            }
        }
        while left != 0 {
            let c = &caps[near[left.trailing_zeros() as usize] as usize];
            left &= left - 1;
            let (n, sd) = c.cone.at(p);
            let margin = Self::margin(base, sunk, c.body);
            if sd < margin {
                p += n * (margin - sd);
            }
        }
        // (Where straight out and back to its length never met, in a fold: out, and where
        // that leaves the segment far off its length, along the surface back to it. Only far
        // off: slid for the few percent a strand folded over a collar is off at rest, the
        // lock below it never comes to rest.)
        if close.surface - p.distance(close.at) <= on_body + 0.01
            && let Some(at) = read(self.boxes, &close.views, close.at, p)
        {
            if at.distance < on_body {
                p += at.out * (on_body - at.distance);
            }
            if ((p - parent).length() / len - 1.0).abs() > 0.08 {
                p = slide(p, at.out, 0.0);
            }
        }
        p
    }

    /// Keep `p` (particle `i`) above the ground.
    #[inline]
    fn above_ground(&self, i: usize, p: Vec3A) -> Vec3A {
        let g = match self.ground.len() {
            0 => return p,
            1 => self.ground[0],
            _ => match self.ground.get(i) {
                Some(g) => *g,
                None => return p,
            },
        };
        let n = Vec3A::new(g[0], g[1], g[2]);
        let d = n.dot(p) + g[3] - 0.002;
        if d < 0.0 { p - n * d } else { p }
    }

    /// Step the piece a cut took off `strand`: free of the head, it falls through the air,
    /// keeps its length, and lies on the body and the ground. Gone ([`TressCut::shed`] 1), its
    /// particles wait at the strand's new end, as do those of pieces cut off before.
    fn piece(
        &self,
        strand: &TressStrand,
        cut: TressCut,
        base: usize,
        pos: &mut [Vec3],
        vel: &mut [Vec3],
    ) {
        let whole = strand.count as usize;
        let first = strand.first as usize - base;
        let (at, end) = (
            (cut.at as usize).min(whole),
            (cut.end as usize).clamp(cut.at as usize, whole),
        );
        let parked = if cut.shed >= 1.0 { at } else { end };
        for i in parked..whole {
            pos[first + i] = pos[first + at - 1];
            vel[first + i] = Vec3::ZERO;
        }
        if parked == at || end - at < 1 {
            return;
        }
        let v = &self.verts[strand.first as usize..strand.first as usize + whole];
        let h = self.dt / self.substeps as f32;
        let air =
            self.carrier_vel + self.wind_dir * (self.wind_strength / self.cfg.wind_feel.max(1e-3));
        let drag = (-PIECE_DRAG * h).exp();
        for s in 0..self.substeps as usize {
            let caps = &self.posed[s * self.caps..(s + 1) * self.caps];
            let before: [Vec3; MAX_VERTS] = std::array::from_fn(|i| {
                if (at..end).contains(&i) {
                    pos[first + i]
                } else {
                    Vec3::ZERO
                }
            });
            for i in at..end {
                let w = air + (vel[first + i] - air) * drag + self.gravity * h;
                pos[first + i] += w * h;
            }
            let mut rubs = [false; MAX_VERTS];
            for _ in 0..2 {
                for i in at + 1..end {
                    let (a, b) = (pos[first + i - 1], pos[first + i]);
                    let d = b - a;
                    let len = d.length();
                    if len > 1e-7 {
                        let corr = d * (0.5 * (len - v[i].rest_len) / len);
                        pos[first + i - 1] += corr;
                        pos[first + i] -= corr;
                    }
                }
                for i in at..end {
                    let mut p = Vec3A::from(pos[first + i]);
                    for c in caps {
                        let (out, d) = c.cone.at(p);
                        if d < 0.002 {
                            p += out * (0.002 - d);
                            rubs[i] = true;
                        }
                    }
                    if !self.boxes.is_empty()
                        && let Some((out, d)) =
                            self.surface(s, u16::MAX >> (16 - self.boxes.len()), p)
                        && d < 0.002
                    {
                        p += out * (0.002 - d);
                        rubs[i] = true;
                    }
                    let on = self.above_ground(strand.first as usize + i, p);
                    rubs[i] |= on != p;
                    pos[first + i] = on.into();
                }
            }
            for i in at..end {
                // (What it lies on holds it: it slides off a shoulder slowly.)
                let hold = if rubs[i] { 0.5 } else { 1.0 };
                vel[first + i] = (pos[first + i] - before[i]) * (hold / h);
            }
        }
    }

    /// Step one strand through every substep. `pos` and `vel` hold particles from `base` on,
    /// `still` how long the strand has lain still.
    fn strand(
        &self,
        strand: &TressStrand,
        cut: TressCut,
        still: &mut f32,
        base: usize,
        pos: &mut [Vec3],
        vel: &mut [Vec3],
    ) {
        let whole = strand.count as usize;
        if !(2..=MAX_VERTS).contains(&whole) {
            return;
        }
        let first = strand.first as usize;
        let frame = self.frames[strand.frame as usize];
        // Cut: what is left of it is the strand; the piece that came off falls by itself.
        let n = if cut.at == 0 {
            whole
        } else {
            let at = (cut.at as usize).min(whole);
            self.piece(strand, cut, base, pos, vel);
            if at < 2 {
                // (Cut at its root: nothing left to swing.)
                let (to_pos, to_rot) = frame.pose(1.0);
                pos[first - base] = to_pos + to_rot * self.verts[first].rest;
                vel[first - base] = Vec3::ZERO;
                return;
            }
            at
        };
        let v = &self.verts[first..first + n];
        let run = first - base..first - base + n;
        if *still >= SLEEPS_AFTER {
            // Asleep: it rides its frame (which has barely moved, or it would be awake).
            let (from_pos, from_rot) = frame.pose(0.0);
            let (to_pos, to_rot) = frame.pose(1.0);
            let turn = to_rot * from_rot.conjugate();
            for i in run {
                pos[i] = to_pos + turn * (pos[i] - from_pos);
                vel[i] = Vec3::ZERO;
            }
            return;
        }
        let cfg = self.cfg;
        let h = self.dt / self.substeps as f32;
        let inv_h = 1.0 / h;
        // Per-substep gains are the source's at its 6 substeps of a 60 Hz frame.
        let per_substep = h * 360.0;
        let u = Vec3A::from(self.carrier_vel);
        let carried = Vec3A::from(self.carried);
        let limit = |vel: Vec3A| {
            let rel = vel - u;
            let speed2 = rel.length_squared();
            if speed2 > MAX_SPEED * MAX_SPEED {
                u + rel * (MAX_SPEED / speed2.sqrt())
            } else {
                vel
            }
        };
        let mut p = [Vec3A::ZERO; MAX_VERTS];
        let mut prev = [Vec3A::ZERO; MAX_VERTS];
        let mut rest = [Vec3A::ZERO; MAX_VERTS];
        let (start_pos, start_rot) = frame.pose(0.0);
        for i in 0..n {
            rest[i] = v[i].rest.into();
            let (x, w) = (pos[run.start + i], vel[run.start + i]);
            if x.is_finite() && w.is_finite() {
                p[i] = x.into();
                prev[i] = (x - w * h).into();
            } else {
                p[i] = (start_pos + start_rot * v[i].rest).into();
                prev[i] = p[i];
            }
        }
        // What holds for each point through the step: its gains, and the air and the hair
        // around it where it starts (both are smooth over the centimeter it moves in a step).
        let mut held = [Held::default(); MAX_VERTS];
        let eff_wind = self.wind_strength;
        let turb =
            cfg.turbulence * (0.22 * eff_wind.min(4.0) + 0.07 * (eff_wind * eff_wind).min(6.0));
        let time = self.time + 0.5 * self.dt;
        // The turbulence at every fourth point (its eddies are decimeters wide, the points
        // two centimeters apart), the others between.
        const EVERY: usize = 4;
        let mut eddies = [Vec3A::ZERO; MAX_VERTS / EVERY + 2];
        if eff_wind > 0.0 && turb > 0.0 {
            let drift = self.wind_dir * (time * (0.45 + 0.35 * eff_wind.min(4.0)));
            for (k, eddy) in eddies[..(n - 1).div_ceil(EVERY) + 1].iter_mut().enumerate() {
                let at = Vec3::from(p[(k * EVERY).min(n - 1)]);
                *eddy = curl_noise(at * cfg.turbulence_freq - drift).into();
            }
        }
        let (c_pos, c_rot) = self.carrier.pose(0.0);
        let to_head = c_rot.conjugate();
        let all = if self.caps >= 32 {
            u32::MAX
        } else {
            (1u32 << self.caps) - 1
        };
        let wet_at = |i: usize| {
            self.wet
                .get(first + i)
                .copied()
                .unwrap_or(0.0)
                .clamp(0.0, 1.0)
        };
        let wet_mean = (0..n).map(wet_at).sum::<f32>() / n as f32;
        for i in 1..n {
            let wet = wet_at(i);
            let k = &mut held[i];
            k.wet = wet;
            k.edge = rest[i] - rest[i - 1];
            k.dir = k.edge.try_normalize().unwrap_or(Vec3A::ZERO);
            let pw = v[i].pin.clamp(0.0, 1.0);
            if pw > 0.001 {
                k.pin = (1.0 - (1.0 - (pw * 0.92).min(0.96)).powf(h * 60.0)).clamp(0.0, 0.85);
            }
            let boost = match i {
                1 => 2.3,
                2 => 1.45,
                _ => 1.0,
            };
            let stiff = (v[i].stiffness * cfg.stiffness).clamp(0.0, 1.0);
            // (Wet, a hair is three times its weight in water for the same stiffness: the
            // grown shape holds half as well.)
            k.shape = (stiff.powf(1.42) * 0.064 * boost * per_substep * (1.0 - WET_LIMP * wet))
                .clamp(0.0, 0.28);
            k.bend = (v[i].bend * cfg.stiffness).clamp(0.0, 1.0);
            for (skip, chord) in k.chord.iter_mut().enumerate() {
                let skip = skip + 1;
                if i >= skip && i + skip < n {
                    *chord = rest[i + skip].distance(rest[i - skip]);
                }
            }
            k.damp = (-(cfg.damping * v[i].damping.max(0.1) * 60.0 + self.settle) * h).exp();
            k.aero = cfg.air_drag * smoothstep(0.0, 0.15, v[i].t) * (1.0 - WET_HEAVY * wet);
            let ramp = smoothstep(0.0, 0.22, v[i].t) * (1.0 - WET_FLAT * wet);
            k.margin = [0.0005 + 0.0022 * ramp, 0.0004 + 0.0018 * ramp];
            k.smooth = v[i].t > 0.16;
            k.near = Self::near(&self.posed[..self.caps], all, p[i], 0.0, Some(self.pads));
            k.boxes = self.near_boxes(p[i]);
            if eff_wind > 0.0 {
                let gust = 1.0
                    + cfg.gust
                        * (0.55 * (time * 2.3 + p[i].x * 3.2 + strand.seed * 1.7).sin()
                            + 0.45 * (time * 4.1 - p[i].z * 2.8 + p[i].y * 1.9).sin());
                let mut wind = Vec3A::from(self.wind_dir) * (eff_wind * 0.68 * gust.max(0.15));
                if turb > 0.0 {
                    let (at, part) = (i / EVERY, (i % EVERY) as f32 / EVERY as f32);
                    let mut curl = eddies[at].lerp(eddies[at + 1], part);
                    // Updrafts are damped: gusts sweep the hair sideways, not up.
                    if curl.y > 0.0 {
                        curl.y *= 0.22;
                    }
                    wind += curl * turb;
                }
                if wind.y > 0.12 {
                    wind.y = 0.12 + (wind.y - 0.12) * 0.15;
                }
                k.wind = wind;
            }
            k.acc = self.gravity.into();
            if cfg.self_collision > 0.001 {
                let (density, grad, mean) = self
                    .grid
                    .sample(to_head * (Vec3::from(p[i]) - c_pos), vel[run.start + i]);
                let mut pressure = -(c_rot * grad / (density + 6.0)) * (0.065 * cfg.self_collision);
                if pressure.y > 0.0 {
                    pressure.y *= 0.12;
                }
                let len = pressure.length();
                if len > 2.8 {
                    pressure *= 2.8 / len;
                }
                // (Wet hair has no air in it to keep its volume, and moves as one.)
                k.acc += Vec3A::from(
                    pressure * (smoothstep(0.08, 0.30, v[i].t) * (1.0 - WET_FLAT * wet)),
                );
                if density > 0.5 && cfg.cohesion > 0.001 {
                    let w = (density / 14.0).clamp(0.0, 1.0)
                        * cfg.cohesion
                        * cfg.self_collision
                        * (1.0 + WET_GATHERS * wet);
                    k.mean = mean.into();
                    k.cohere = (w * h * 10.0).clamp(0.0, 0.28);
                }
            }
        }
        let inv_aniso = 1.0 / cfg.aniso.max(1.0);
        let iterations = cfg.iterations.clamp(1, 8);
        let settle = (-self.settle * h).exp();
        let mut close = [Close::default(); MAX_VERTS];
        let mut grip = [Grip::default(); MAX_VERTS];
        let mut was = [Vec3A::ZERO; MAX_VERTS];
        let seg = (1..n).map(|i| held[i].edge.length()).sum::<f32>() / (n - 1) as f32;
        let inner = cfg.inner_friction.max(0.0) * (1.0 + WET_GATHERS * wet_mean) * h
            / (seg * seg).max(1e-8);
        for s in 0..self.substeps {
            let frac = (s + 1) as f32 / self.substeps as f32;
            let (h_pos, h_quat) = frame.pose(frac);
            let h_rot = Mat3A::from_quat(h_quat);
            let caps = &self.posed[s as usize * self.caps..(s as usize + 1) * self.caps];
            // 1. The root rides the frame.
            let h_pos = Vec3A::from(h_pos);
            let root = h_pos + h_rot * rest[0];
            // 0. Friction inside the lock: every point's velocity spread along the strand
            //    from the root's (an implicit step of its diffusion, solved down and up the
            //    strand).
            if inner > 0.0 {
                let mut w = [Vec3A::ZERO; MAX_VERTS];
                let mut c = [0.0f32; MAX_VERTS];
                w[0] = (root - (p[0] + carried)) * inv_h;
                for i in 1..n {
                    let (b, up) = if i == n - 1 {
                        (1.0 + inner, 0.0)
                    } else {
                        (1.0 + 2.0 * inner, -inner)
                    };
                    let denom = b + inner * c[i - 1];
                    c[i] = up / denom;
                    w[i] = ((p[i] - prev[i]) * inv_h + w[i - 1] * inner) / denom;
                }
                for i in (1..n - 1).rev() {
                    w[i] -= w[i + 1] * c[i];
                }
                for i in 1..n {
                    prev[i] = p[i] - w[i] * h;
                }
            }
            p[0] = root;
            prev[0] = p[0];
            // 2. Forces and prediction.
            for i in 1..n {
                let k = &held[i];
                was[i] = p[i];
                grip[i].pressed = 0.0;
                p[i] += carried;
                prev[i] += carried;
                let mut vel = limit((p[i] - prev[i]) * inv_h);
                let tangent = (p[i] - p[i - 1]).try_normalize().unwrap_or(Vec3A::NEG_Y);
                let rel = k.wind - (vel - u);
                let along = tangent * rel.dot(tangent);
                let eff = (rel - along) + along * inv_aniso;
                let mut aero = eff * (k.aero * (0.45 + 0.50 * eff.length().min(6.5)));
                aero.y = aero.y.min(2.5);
                let len2 = aero.length_squared();
                if len2 > 24.0 * 24.0 {
                    aero *= 24.0 / len2.sqrt();
                }
                if k.cohere > 0.0 {
                    vel = vel.lerp(k.mean, k.cohere);
                }
                vel = u + (vel - u) * k.damp;
                prev[i] = p[i];
                p[i] += vel * h + (k.acc + aero) * (h * h);
            }
            // 3. The head carries the first points softly.
            for i in 1..n {
                if held[i].pin > 0.0 {
                    p[i] = p[i].lerp(h_pos + h_rot * rest[i], held[i].pin);
                }
            }
            // 4. Every segment turns toward its grown direction relative to the one before
            //    (70% of the way of the segment before's turn from how it was grown).
            for i in 1..n {
                let k = held[i].shape;
                if k <= 1e-5 {
                    continue;
                }
                let mut edge = h_rot * held[i].edge;
                if i >= 2
                    && held[i - 1].dir != Vec3A::ZERO
                    && let Some(now) = (p[i - 1] - p[i - 2]).try_normalize()
                {
                    edge = turned(edge, h_rot * held[i - 1].dir, now, 0.70);
                }
                let diff = (p[i - 1] + edge - p[i]) * k;
                if i >= 2 {
                    p[i - 1] -= diff * 0.5;
                    p[i] += diff * 0.5;
                } else {
                    p[i] += diff;
                }
            }
            // The capsules each point can touch in this substep.
            for i in 1..n {
                close[i] = if held[i].near == 0 {
                    Close {
                        at: p[i],
                        ..Close::default()
                    }
                } else {
                    self.close(caps, held[i].near, p[i], held[i].margin[0], v[i].sunk)
                };
                (close[i].views, close[i].surface) = if held[i].boxes == 0 {
                    (Views::default(), f32::MAX)
                } else {
                    let posed = &self.posed_boxes
                        [s as usize * self.boxes.len()..(s as usize + 1) * self.boxes.len()];
                    let views =
                        views(self.boxes, posed, held[i].boxes, p[i], REACHED, usize::MAX).1;
                    let at = read(self.boxes, &views, p[i], p[i]);
                    (views, at.map_or(f32::MAX, |at| at.distance))
                };
            }
            // 5. Bends, lengths and the body.
            for it in 0..iterations {
                if it == 0 {
                    for skip in [1usize, 2] {
                        let gain = if skip == 1 { 0.082 } else { 0.060 };
                        if n < 2 * skip + 1 {
                            continue;
                        }
                        for (i, k) in held.iter().enumerate().take(n - skip).skip(skip) {
                            let kb = k.bend;
                            if kb < 0.01 {
                                continue;
                            }
                            let (a, b) = (i - skip, i + skip);
                            let chord = k.chord[skip - 1];
                            let d = p[b] - p[a];
                            let len = d.length();
                            if len > 1e-6 && chord > 1e-6 {
                                let corr = d * ((len - chord) / len) * (gain * kb);
                                // The root doesn't move; the other end takes it all.
                                if a == 0 {
                                    p[b] -= corr;
                                } else {
                                    p[a] += corr * 0.5;
                                    p[b] -= corr * 0.5;
                                }
                            }
                        }
                    }
                }
                for i in 1..n {
                    let len = v[i].rest_len;
                    let delta = p[i] - p[i - 1];
                    let dist = delta.length();
                    if dist > 1e-7 {
                        if it + 1 < iterations {
                            let corr = delta * ((dist - len) / dist);
                            if i >= 2 {
                                p[i - 1] += corr * 0.5;
                                p[i] -= corr * 0.5;
                            } else {
                                p[i] -= corr;
                            }
                        } else {
                            p[i] = p[i - 1] + delta * (len / dist);
                        }
                    } else {
                        let dir = if held[i].dir == Vec3A::ZERO {
                            Vec3A::NEG_Y
                        } else {
                            held[i].dir
                        };
                        p[i] = p[i - 1] + h_rot * dir * len;
                    }
                }
                for i in 1..n {
                    if close[i].n != 0 || close[i].surface < REACHED {
                        (p[i], prev[i]) = self.resolve(
                            caps,
                            &close[i],
                            p[i],
                            prev[i],
                            held[i].margin[0],
                            v[i].sunk,
                            held[i].smooth,
                            held[i].wet,
                            &mut grip[i],
                        );
                    }
                    p[i] = self.above_ground(first + i, p[i]);
                }
            }
            // 6. Exact lengths outside every capsule; the snap adds no speed.
            for i in 1..n {
                let dir = if held[i].dir == Vec3A::ZERO {
                    Vec3A::NEG_Y
                } else {
                    held[i].dir
                };
                let exact = self.exact(
                    caps,
                    &close[i],
                    p[i],
                    p[i - 1],
                    v[i].rest_len,
                    held[i].margin[1],
                    v[i].sunk,
                    h_rot * dir,
                );
                prev[i] += exact - p[i];
                p[i] = exact;
            }
            //    What lies on the body grips it: carried along by the bone under it, it
            //    slides only as far as what presses it on lets it.
            let parts = self.boxes.len();
            for i in 1..n {
                let Grip { pressed, n, part } = grip[i];
                if pressed <= 0.0 {
                    continue;
                }
                let part = part as usize;
                let to = &self.posed_boxes[s as usize * parts + part];
                let from = match s {
                    0 => &self.start_boxes[part],
                    s => &self.posed_boxes[(s as usize - 1) * parts + part],
                };
                let stuck = to.pos + to.rot * (from.rot.transpose() * (was[i] - from.pos));
                let slip = p[i] - stuck;
                let slip = slip - n * slip.dot(n);
                let far = slip.length();
                if far < 1e-9 {
                    continue;
                }
                let holds = ((STICKS + WET_CLINGS * held[i].wet) * pressed).min(GRIPS_MOST);
                if far <= holds {
                    // Held: it moves as the skin under it does.
                    p[i] -= slip;
                    let (moved, ride) = (p[i] - prev[i], stuck - was[i]);
                    prev[i] += (moved - n * moved.dot(n)) - (ride - n * ride.dot(n));
                } else {
                    p[i] -= slip * (holds * (SLIDES / STICKS) / far);
                }
            }
            // 7. Velocities: a little of the segment before's (no chatter at the tips), the
            //    speed limit, and rest for what barely moves.
            let mut before = u;
            for i in 1..n {
                let mut w = limit(((p[i] - prev[i]) * inv_h).lerp(before, 0.045));
                w = u + (w - u) * settle;
                if (w - u).length_squared() < 0.03 * 0.03 {
                    w = u + (w - u) * 0.82;
                }
                prev[i] = p[i] - w * h;
                before = w;
            }
        }
        let (end_pos, end_rot) = frame.pose(1.0);
        let mut fastest = 0.0f32;
        for i in 1..n {
            fastest = fastest.max(((p[i] - prev[i]) * inv_h - u).length_squared());
        }
        // A body settling into place under its damping is slow all the way: no rest for it
        // halfway.
        *still = if fastest < RESTFUL * RESTFUL && self.settle <= 0.0 {
            *still + self.dt
        } else {
            0.0
        };
        for i in 0..n {
            let (mut x, mut w) = (Vec3::from(p[i]), Vec3::from((p[i] - prev[i]) * inv_h));
            if !x.is_finite() || (x - end_pos).length_squared() > 25.0 {
                x = end_pos + end_rot * v[i].rest;
                w = self.carrier_vel;
            }
            if !w.is_finite() {
                w = self.carrier_vel;
            }
            pos[run.start + i] = x;
            vel[run.start + i] = if i == 0 { Vec3::ZERO } else { w };
        }
    }
}

/// The capsules a point could touch in one substep's constraint passes: how far off each was
/// when the point was at `at`.
#[derive(Clone, Copy, Default)]
struct Close {
    at: Vec3A,
    n: u8,
    cap: [u8; 6],
    sd: [f32; 6],
    /// How the body's boxes see the point in the substep, and how far off the measured
    /// surface was (`f32::MAX` out of reach).
    views: Views,
    surface: f32,
}

/// How a point is pressed on the body's measured surface in one substep: how far the surface
/// has pushed it out in all, along which way last, and the box whose skin it is.
#[derive(Clone, Copy, Default)]
struct Grip {
    pressed: f32,
    n: Vec3A,
    part: u8,
}

/// What holds for a point of a strand through one step.
#[derive(Clone, Copy, Default)]
struct Held {
    /// The segment to the point before as grown, in the strand's frame, and its direction
    /// (zero without one).
    edge: Vec3A,
    dir: Vec3A,
    /// Per-substep gains: toward where the head carries the point, toward its grown
    /// direction, on its bends; what is left of its speed; its share of the air's drag.
    pin: f32,
    shape: f32,
    bend: f32,
    damp: f32,
    aero: f32,
    /// As-grown distances between the points one and two to either side.
    chord: [f32; 2],
    /// Clearances kept from the capsules: sliding off them, and in the last exact pass.
    margin: [f32; 2],
    /// Far enough down the strand to feel the body's creases blended.
    smooth: bool,
    /// The capsules it can reach within the step, and the body's boxes.
    near: u32,
    boxes: u16,
    /// How wet it is, 0..1.
    wet: f32,
    /// Gravity and the hair's pressure, the wind there, and the velocity of the hair around
    /// it with how much of it the point takes on each substep.
    acc: Vec3A,
    wind: Vec3A,
    mean: Vec3A,
    cohere: f32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::solver::Frame;

    /// A head of straight strands hanging from a ring around a sphere, with a neck below.
    fn head(strands: usize, points: usize) -> (Sim, Tress) {
        let mut sim = Sim::new(DVec3::ZERO);
        let frame = sim.add_frame(Vec3::ZERO, Quat::IDENTITY);
        let mut tress = Tress {
            carrier: frame,
            ..Tress::default()
        };
        tress.capsules.push(TressCapsule {
            frame,
            a: Vec3::ZERO,
            ra: 0.09,
            b: Vec3::new(0.0, -0.02, 0.0),
            rb: 0.085,
            friction: 0.22,
            body: false,
        });
        tress.capsules.push(TressCapsule {
            frame,
            a: Vec3::new(0.0, -0.08, 0.0),
            ra: 0.05,
            b: Vec3::new(0.0, -0.2, 0.0),
            rb: 0.06,
            friction: 0.3,
            body: true,
        });
        let seg = 0.02;
        for s in 0..strands {
            let angle = s as f32 / strands as f32 * std::f32::consts::TAU;
            let root = Vec3::new(angle.cos(), 0.25, angle.sin()).normalize() * 0.092;
            let first = sim.len() as u32;
            for k in 0..points {
                // Out a little, then down.
                let rest = root
                    + Vec3::new(root.x, 0.0, root.z).normalize() * (0.01 * k.min(2) as f32)
                    - Vec3::Y * (seg * k as f32);
                let rest = if k == 0 { root } else { rest };
                sim.add_particle(rest, 0.001);
                let t = k as f32 / (points - 1) as f32;
                tress.verts.push(TressVertex {
                    rest,
                    rest_len: 0.0,
                    t,
                    stiffness: 0.5 - 0.36 * t,
                    bend: 0.46 - 0.3 * t,
                    pin: match k {
                        0 => 1.0,
                        1 => 0.38,
                        2 => 0.12,
                        _ => 0.0,
                    },
                    damping: 1.0,
                    sunk: [0.0; 2],
                });
            }
            for k in 1..points {
                let i = first as usize + k;
                tress.verts[i].rest_len = tress.verts[i].rest.distance(tress.verts[i - 1].rest);
            }
            sim.pin(first, frame, root);
            tress.strands.push(TressStrand {
                first,
                count: points as u32,
                frame,
                seed: s as f32 * 0.37 % 1.0,
            });
        }
        (sim, tress)
    }

    fn stretch(sim: &Sim, tress: &Tress) -> f32 {
        let mut worst = 0.0f32;
        for s in &tress.strands {
            for i in s.first as usize + 1..(s.first + s.count) as usize {
                let len = sim.pos[i].distance(sim.pos[i - 1]);
                worst = worst.max((len / tress.verts[i].rest_len - 1.0).abs());
            }
        }
        worst
    }

    fn deepest(sim: &Sim, tress: &Tress) -> f32 {
        let mut deepest = f32::MAX;
        for (i, p) in sim.pos.iter().enumerate() {
            if tress.verts[i].t == 0.0 {
                continue;
            }
            for c in &tress.capsules {
                let f = sim.frames[c.frame as usize];
                let (a, b) = (f.to_pos + f.to_rot * c.a, f.to_pos + f.to_rot * c.b);
                deepest = deepest.min(tapered_capsule(*p, a, b, c.ra, c.rb).1);
            }
        }
        deepest
    }

    /// A box of samples 8 mm apart from `lo` to `hi` riding `frame`: `at` gives the distance
    /// (m) and the weight at each.
    fn measured(frame: u16, lo: Vec3, hi: Vec3, at: impl Fn(Vec3) -> (f32, u8)) -> TressBox {
        let cell = 0.008;
        let size = ((hi - lo) / cell).ceil() + Vec3::ONE;
        let size = [size.x as u16, size.y as u16, size.z as u16];
        let mut cells = Vec::new();
        for z in 0..size[2] {
            for y in 0..size[1] {
                for x in 0..size[0] {
                    let (d, w) = at(lo + Vec3::new(x as f32, y as f32, z as f32) * cell);
                    cells.push([(d / engine_world::HairBody::UNIT) as i16, w as i16]);
                }
            }
        }
        TressBox {
            frame,
            min: lo,
            cell,
            size,
            cells: cells.into(),
        }
    }

    /// [`head`] with its head and neck measured instead of fitted with capsules, and under
    /// them a pair of shoulders on a frame of their own that stays where it is: each box
    /// holds the distance to the whole of it and how much of the nearest is its own frame's.
    fn measured_head(strands: usize, points: usize) -> (Sim, Tress, Vec<TressCapsule>) {
        let (mut sim, mut tress) = head(strands, points);
        let chest = sim.add_frame(Vec3::ZERO, Quat::IDENTITY);
        let mut shapes = std::mem::take(&mut tress.capsules);
        shapes.push(TressCapsule {
            frame: chest,
            a: Vec3::new(-0.16, -0.24, 0.0),
            ra: 0.05,
            b: Vec3::new(0.16, -0.24, 0.0),
            rb: 0.05,
            friction: 0.3,
            body: true,
        });
        // (Each box: the distance to the whole body as it stands here, and how much of the
        // nearest of it is its own frame's: all of it but where the two meet.)
        let own = |of: u16| {
            let shapes = shapes.clone();
            move |p: Vec3| {
                let to = |own: bool| {
                    shapes
                        .iter()
                        .filter(|c| (c.frame == of) == own)
                        .map(|c| tapered_capsule(p, c.a, c.b, c.ra, c.rb).1)
                        .fold(f32::MAX, f32::min)
                };
                let (own, other) = (to(true), to(false));
                let share = (0.5 + (other - own) / 0.1).clamp(0.0, 1.0);
                (own.min(other), (share * 255.0) as u8)
            }
        };
        tress.boxes.push(measured(
            0,
            Vec3::new(-0.14, -0.3, -0.14),
            Vec3::new(0.14, 0.14, 0.14),
            own(0),
        ));
        tress.boxes.push(measured(
            chest,
            Vec3::new(-0.26, -0.34, -0.1),
            Vec3::new(0.26, -0.14, 0.1),
            own(chest),
        ));
        (sim, tress, shapes)
    }

    /// The source project's head shake: 6.8 rad/s, 45 degrees each way, in bursts.
    fn shake(time: f32) -> (Vec3, Quat) {
        let env = (0.9 * time).sin().abs().powf(0.4);
        let shake = (6.8 * time).sin() * env;
        let rot = Quat::from_euler(
            EulerRot::YXZ,
            0.78 * shake,
            0.14 * (3.4 * time).sin(),
            0.24 * (6.8 * time + 0.6).sin() * env,
        );
        (
            Vec3::new(0.018 * shake, 0.006 * (6.8 * time).cos().abs(), 0.0),
            rot,
        )
    }

    #[test]
    fn round_cone_distance_matches_spheres_along_the_axis() {
        let (a, b) = (Vec3::ZERO, Vec3::new(0.0, -0.3, 0.0));
        // Beside the middle of an untapered capsule.
        let (n, d) = tapered_capsule(Vec3::new(0.2, -0.15, 0.0), a, b, 0.05, 0.05);
        assert!((d - 0.15).abs() < 1e-5 && (n - Vec3::X).length() < 1e-4);
        // Beyond each end: the end's sphere.
        let (n, d) = tapered_capsule(Vec3::new(0.0, 0.2, 0.0), a, b, 0.05, 0.02);
        assert!((d - 0.15).abs() < 1e-5 && (n - Vec3::Y).length() < 1e-4);
        let (_, d) = tapered_capsule(Vec3::new(0.0, -0.5, 0.0), a, b, 0.05, 0.02);
        assert!((d - 0.18).abs() < 1e-5);
        // Tapered: never further out than the linearly tapered spheres along the axis.
        for k in 0..50 {
            let p = Vec3::new(0.03 + 0.002 * k as f32, -0.006 * k as f32, 0.01);
            let (_, d) = tapered_capsule(p, a, b, 0.05, 0.02);
            let spheres = (0..=100)
                .map(|s| {
                    let t = s as f32 / 100.0;
                    p.distance(a.lerp(b, t)) - (0.05 + (0.02 - 0.05) * t)
                })
                .fold(f32::MAX, f32::min);
            assert!(d <= spheres + 1e-4, "{d} {spheres}");
        }
    }

    #[test]
    fn hair_keeps_its_length_and_stays_out_of_the_head_through_a_head_shake() {
        let (mut sim, mut tress) = head(48, 20);
        let dt = 1.0 / 60.0;
        let mut worst_stretch = 0.0f32;
        let mut deepest_in = f32::MAX;
        let mut swung = 0.0f32;
        for step in 0..600 {
            let (pos, rot) = shake(step as f32 * dt);
            sim.set_frame(0, pos, rot);
            tress.step(&mut sim, dt);
            sim.params.time += dt;
            for f in &mut sim.frames {
                (f.from_pos, f.from_rot) = (f.to_pos, f.to_rot);
            }
            assert!(sim.pos.iter().all(|p| p.is_finite()));
            worst_stretch = worst_stretch.max(stretch(&sim, &tress));
            deepest_in = deepest_in.min(deepest(&sim, &tress));
            // How far the tips are from where the head alone would carry them.
            for s in &tress.strands {
                let tip = (s.first + s.count - 1) as usize;
                swung = swung.max(sim.pos[tip].distance(pos + rot * tress.verts[tip].rest));
            }
        }
        assert!(worst_stretch < 0.015, "stretched {worst_stretch}");
        assert!(deepest_in > -1e-4, "{deepest_in} m inside a capsule");
        assert!(swung > 0.05, "the hair swings in a shake: {swung} m");
    }

    #[test]
    fn hair_stays_out_of_a_measured_body_whose_parts_ride_their_own_bones() {
        let (mut sim, mut tress, shapes) = measured_head(48, 20);
        assert!(tress.capsules.is_empty() && tress.fits(&sim));
        let dt = 1.0 / 60.0;
        let mut worst_stretch = 0.0f32;
        let mut deepest_in = f32::MAX;
        let mut on_shoulders = 0usize;
        for step in 0..600 {
            // The head shakes on shoulders that stay.
            let (pos, rot) = shake(step as f32 * dt);
            sim.set_frame(0, pos, rot);
            tress.step(&mut sim, dt);
            sim.params.time += dt;
            for f in &mut sim.frames {
                (f.from_pos, f.from_rot) = (f.to_pos, f.to_rot);
            }
            assert!(sim.pos.iter().all(|p| p.is_finite()));
            worst_stretch = worst_stretch.max(stretch(&sim, &tress));
            for (i, p) in sim.pos.iter().enumerate() {
                if tress.verts[i].t == 0.0 {
                    continue;
                }
                for c in &shapes {
                    let f = sim.frames[c.frame as usize];
                    let (a, b) = (f.to_pos + f.to_rot * c.a, f.to_pos + f.to_rot * c.b);
                    let d = tapered_capsule(*p, a, b, c.ra, c.rb).1;
                    on_shoulders += (c.frame != 0 && d < 0.006) as usize;
                    // (Where the frame carries all of the body nearest: where the two
                    // share it, it has gone between them, as skin does.)
                    let was = f.to_rot.inverse() * (*p - f.to_pos);
                    let others = shapes
                        .iter()
                        .filter(|o| o.frame != c.frame)
                        .map(|o| tapered_capsule(was, o.a, o.b, o.ra, o.rb).1)
                        .fold(f32::MAX, f32::min);
                    if others - d >= 0.05 {
                        deepest_in = deepest_in.min(d);
                    }
                }
            }
        }
        // (Where a hard shake presses a segment into the fold of neck and shoulder, staying out
        // comes before its exact length: half a millimeter of a segment's 15.)
        assert!(worst_stretch < 0.04, "stretched {worst_stretch}");
        // (The samples are 8 mm apart: between them the surface is known to a few tenths
        // of a millimeter.)
        assert!(deepest_in > -5e-4, "{deepest_in} m inside the body");
        assert!(
            on_shoulders > 100,
            "the hair lies on the shoulders: {on_shoulders}"
        );
        // The surface is there to be asked afterwards, with whose it is.
        let crown = sim.frames[0].to_pos + sim.frames[0].to_rot * Vec3::new(0.0, 0.1, 0.0);
        let (n, d, head) = tress.surface_at(&sim.frames, crown).unwrap();
        assert!(
            (d - 0.01).abs() < 1e-3 && n.y > 0.9 && head > 0.99,
            "{n} {d} {head}"
        );
        assert!(
            tress
                .surface_at(&sim.frames, Vec3::new(0.0, 1.0, 0.0))
                .is_none()
        );
    }

    #[test]
    fn a_cut_strand_keeps_what_is_left_and_its_piece_falls_and_is_gone() {
        let (mut sim, mut tress) = head(32, 16);
        for _ in 0..120 {
            tress.step(&mut sim, 1.0 / 60.0);
        }
        let s = tress.strands[5];
        let first = s.first as usize;
        let hung: Vec<Vec3> = sim.pos[first..first + 16].to_vec();
        // Not at the root, not past the tip; once cut, only nearer the root.
        assert!(!tress.cut(5, 0, 0.5) && !tress.cut(5, 16, 0.5));
        assert!(tress.cut(5, 9, 0.25));
        assert!(!tress.cut(5, 12, 0.5));
        let left = tress.left(5);
        assert!((0.5..0.6).contains(&left), "{left}");
        assert_eq!(tress.left(4), 1.0);
        for _ in 0..30 {
            tress.step(&mut sim, 1.0 / 60.0);
        }
        // What is left hangs as it hung; the piece has fallen, in one piece.
        for (i, hung) in hung.iter().enumerate() {
            let now = sim.pos[first + i];
            if i < 9 {
                assert!(now.distance(*hung) < 0.02, "point {i}");
            } else {
                assert!(now.y < hung.y - 0.3, "point {i} fell to {}", now.y);
            }
        }
        for i in 10..16 {
            let len = sim.pos[first + i].distance(sim.pos[first + i - 1]);
            assert!(
                (len / tress.verts[first + i].rest_len - 1.0).abs() < 0.1,
                "segment {i}: {len}"
            );
        }
        assert!(tress.cuts[5].shed > 0.1 && tress.cuts[5].shed < 0.2);
        // The other strands never noticed.
        for (k, s) in tress.strands.iter().enumerate().filter(|(k, _)| *k != 5) {
            for i in s.first as usize + 1..(s.first + s.count) as usize {
                let len = sim.pos[i].distance(sim.pos[i - 1]);
                assert!(
                    (len / tress.verts[i].rest_len - 1.0).abs() < 0.02,
                    "strand {k}"
                );
            }
        }
        for _ in 0..(SHED_SECS * 60.0) as usize {
            tress.step(&mut sim, 1.0 / 60.0);
        }
        assert_eq!(tress.cuts[5].shed, 1.0);
        // Gone: its particles wait at the new end, and a second cut takes off only what is
        // left of the strand.
        for i in 9..16 {
            assert!(sim.pos[first + i].distance(sim.pos[first + 8]) < 0.02);
        }
        assert!(tress.cut(5, 4, 0.0));
        assert_eq!((tress.cuts[5].at, tress.cuts[5].end), (4, 9));
        for _ in 0..10 {
            tress.step(&mut sim, 1.0 / 60.0);
        }
        assert!(sim.pos.iter().all(|p| p.is_finite()));
        assert!(sim.pos[first + 6].y < hung[6].y - 0.05);
    }

    #[test]
    fn hair_comes_to_rest_and_blows_downwind() {
        let (mut sim, mut tress) = head(32, 20);
        let dt = 1.0 / 60.0;
        let run = |sim: &mut Sim, tress: &mut Tress, steps: usize, wind: Vec3| {
            sim.params.wind = wind;
            for _ in 0..steps {
                tress.step(sim, dt);
                sim.params.time += dt;
            }
        };
        run(&mut sim, &mut tress, 300, Vec3::ZERO);
        let fastest = sim.vel.iter().fold(0.0f32, |m, v| m.max(v.length()));
        assert!(fastest < 0.02, "still air: at rest ({fastest} m/s)");
        let mean_x = |sim: &Sim| sim.pos.iter().map(|p| p.x).sum::<f32>() / sim.len() as f32;
        let calm = mean_x(&sim);
        run(&mut sim, &mut tress, 240, Vec3::new(5.0, 0.0, 0.0));
        let blown = mean_x(&sim);
        assert!(
            blown > calm + 0.01,
            "a 5 m/s wind moves the hair: {calm} {blown}"
        );
        // It hangs, even in a gale: not flat out, not over the head.
        run(&mut sim, &mut tress, 240, Vec3::new(14.0, 0.0, 0.0));
        let tips_up = tress
            .strands
            .iter()
            .filter(|s| sim.pos[(s.first + s.count - 1) as usize].y > 0.05)
            .count();
        assert!(
            tips_up < tress.strands.len() / 4,
            "{tips_up} tips above the head"
        );
        assert!(deepest(&sim, &tress) > -1e-4);
        run(&mut sim, &mut tress, 900, Vec3::ZERO);
        assert!(
            (mean_x(&sim) - calm).abs() < 0.01,
            "and falls back: {} {calm}",
            mean_x(&sim)
        );
    }

    /// `cargo test -p engine_softbody --release --lib tress::tests::cost -- --ignored --nocapture`
    #[test]
    #[ignore = "a measurement"]
    fn cost_of_a_head_of_hair() {
        let (mut sim, mut tress) = head(300, 20);
        // As many capsules as a person has: shoulders, arms and a torso under the neck.
        for k in 0..20 {
            let side = if k % 2 == 0 { -1.0 } else { 1.0 };
            let row = (k / 2) as f32;
            tress.capsules.push(TressCapsule {
                frame: 0,
                a: Vec3::new(side * 0.04, -0.2 - 0.02 * row, 0.02 * (row - 4.0)),
                ra: 0.05,
                b: Vec3::new(side * (0.1 + 0.01 * row), -0.24 - 0.02 * row, 0.0),
                rb: 0.045,
                friction: 0.4,
                body: true,
            });
        }
        let dt = 1.0 / 60.0;
        sim.params.wind = Vec3::new(3.0, 0.0, 0.0);
        let started = std::time::Instant::now();
        for step in 0..300 {
            let (pos, rot) = shake(step as f32 * dt);
            sim.set_frame(0, pos, rot);
            tress.step(&mut sim, dt);
            sim.params.time += dt;
            for f in &mut sim.frames {
                (f.from_pos, f.from_rot) = (f.to_pos, f.to_rot);
            }
        }
        println!(
            "{} strands, {} points: {:.2} ms a step on {} threads",
            tress.strands.len(),
            sim.len(),
            started.elapsed().as_secs_f64() * 1000.0 / 300.0,
            leaf_pool().thread_num()
        );
    }

    #[test]
    fn a_moving_head_takes_its_hair_along() {
        let (mut sim, mut tress) = head(24, 16);
        let dt = 1.0 / 60.0;
        // A dash: 20 m/s within a step, held for a second.
        let mut at = Vec3::ZERO;
        for step in 0..90 {
            let v = if step >= 10 { 20.0 } else { 0.0 };
            at += Vec3::new(0.0, 0.0, v * dt);
            sim.frames[0] = Frame {
                from_pos: sim.frames[0].to_pos,
                from_rot: Quat::IDENTITY,
                to_pos: at,
                to_rot: Quat::IDENTITY,
            };
            tress.step(&mut sim, dt);
            sim.params.time += dt;
            assert!(stretch(&sim, &tress) < 0.02, "step {step}");
        }
        assert!(deepest(&sim, &tress) > -1e-4);
        // Streaming behind, attached.
        let behind = sim.pos.iter().map(|p| p.z - at.z).sum::<f32>() / sim.len() as f32;
        assert!(behind < -0.01 && behind > -0.5, "{behind}");
    }
}
