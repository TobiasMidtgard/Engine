//! Dry-stone walls between the fields of the valley floor: rough field stones laid dry in
//! uneven courses that lean in toward the top, big stones at the foot and smaller ones above,
//! little pinning stones wedged into the gaps, through-stones tying the two faces together, a
//! dark rubble core showing in the chinks and a coping of stones set on edge. A run of wall
//! ends in a head: stones laid across the whole wall, so the end is built, not cut.
//!
//! A wall is a row of straight segments. Every segment is one of a few variants, each built as
//! meshes the scenery shares: the stones, every one a weathered rock of its own
//! ([`engine::render::rock::rock`]: break faces, rounded edges, pitting, dirt in the cracks),
//! dense enough that the renderer draws a segment as virtual geometry, picking each cluster's
//! detail by its size on screen; and the core, a tapered prism just inside the stones: dark up
//! close (the shadowed rubble between the stones), stone-colored from afar, where it is the
//! whole wall.
//!
//! Up close the merged rocks give way to hero stones ([`Placed`]): every stone as its own
//! entity, one of a small palette of dressed building stones ([`HERO_CLASSES`], Rendering's
//! [`engine::render::rock::WallStone`]: split faces, chipped arrises, lichen and grime, real
//! displaced relief), stretched to the stone's box.
//!
//! Build frame: the segment runs along X from `-SEGMENT / 2` to `SEGMENT / 2`, centered on
//! Z = 0, with the ground at y = 0 (the foot goes [`SINK`] meters below it). A head is built
//! with its outer face at X = 0, the wall it ends running off toward -X.

use crate::village::Mb;
use engine::core::math::{hash01, smoothstep};
use engine::prelude::*;
use engine::render::rock::StoneKind;

/// Length of one segment, m. Walls stretch theirs a little to fit a run exactly.
pub const SEGMENT: f32 = 5.0;
/// Depth of the foot below the ground, so uneven ground never shows a gap under the wall.
pub const SINK: f32 = 0.2;
/// Width at the ground and under the coping, m.
const FOOT: f32 = 0.8;
const TOP: f32 = 0.46;
/// Height of the body under the coping, m.
const BODY: f32 = 0.92;
/// Height of the coping stones (the tallest), m.
const COPING: f32 = 0.3;
/// Overall height above the ground, m.
pub const HEIGHT: f32 = BODY + COPING;
/// Half the width at the foot (for colliders and clearances), m.
pub const HALF_FOOT: f32 = FOOT * 0.5;
/// How far a head reaches back along the wall from its outer face, m.
pub const HEAD_DEPTH: f32 = 0.55;

/// One segment variant's meshes.
pub struct WallSegment {
    /// The stones (virtual geometry: the renderer picks their detail).
    pub near: Mesh,
    /// Where each of `near`'s stones is, for the hero stones up close.
    pub placed: Vec<Placed>,
    /// The last segment of a run: its stones stop short of the +X end, where the head goes.
    pub near_end: Mesh,
    pub placed_end: Vec<Placed>,
    /// The core between the faces: dark, for behind the stones. Open at its ends, which the
    /// neighbors close.
    pub core: Mesh,
    /// The dark core of a run's last segment: its +X end stops inside the head's stones.
    pub core_end: Mesh,
    /// The core stone-colored, the whole wall from afar.
    pub core_far: Mesh,
}

/// A wall head's stones.
pub struct WallHead {
    pub near: Mesh,
    pub placed: Vec<Placed>,
}

/// Hero stone classes: the kind of stone and the size its palette meshes are built at
/// (along the wall or the stone's own x, up, into the wall). A placed stone takes the class
/// for its part of the wall nearest its size and is stretched to fit.
pub const HERO_CLASSES: [(StoneKind, Vec3); 8] = [
    // Face stones, long to small, and the pinning stones.
    (StoneKind::SplitFace, Vec3::new(0.6, 0.26, 0.3)),
    (StoneKind::SplitFace, Vec3::new(0.4, 0.2, 0.28)),
    (StoneKind::SplitFace, Vec3::new(0.25, 0.15, 0.25)),
    (StoneKind::SplitFace, Vec3::new(0.12, 0.06, 0.12)),
    // Coping stones on edge.
    (StoneKind::SplitFace, Vec3::new(0.11, 0.24, 0.5)),
    // Through-stones: bedded slabs across the wall.
    (StoneKind::Slab, Vec3::new(0.32, 0.15, 0.9)),
    // A head's capstone.
    (StoneKind::Slab, Vec3::new(0.52, 0.19, 0.54)),
    // A head's courses, laid across the wall.
    (StoneKind::SplitFace, Vec3::new(0.4, 0.22, 0.8)),
];

/// Palette meshes per hero class (different stones of the same size).
pub const HERO_VARIANTS: usize = 4;

/// What part of the wall a stone is.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Role {
    Face,
    Coping,
    Through,
    Cap,
    HeadCourse,
}

/// A stone as laid: its center and axes (along, up, out of the face; a reflection on one face
/// of the wall) in the build frame, its box, its palette class and variant, and a shade.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    pub center: Vec3,
    pub ax: [Vec3; 3],
    pub size: Vec3,
    pub class: u8,
    pub variant: u8,
    pub shade: u8,
}

/// Hero stone shades (a material per shade multiplies the palette's own colors).
pub const HERO_SHADES: usize = 4;

/// The class for a stone of `size` in `role`.
fn hero_class(role: Role, size: Vec3) -> u8 {
    match role {
        Role::Face => (0..4)
            .min_by(|&a, &b| {
                let miss = |c: usize| {
                    let r = size / HERO_CLASSES[c].1;
                    r.x.ln().abs() + r.y.ln().abs() + r.z.ln().abs()
                };
                miss(a).total_cmp(&miss(b))
            })
            .unwrap_or(1) as u8,
        Role::Coping => 4,
        Role::Through => 5,
        Role::Cap => 6,
        Role::HeadCourse => 7,
    }
}

/// Stones being laid: the merged rocks and where each stone went.
struct Laying {
    mb: Mb,
    placed: Vec<Placed>,
}

impl Laying {
    fn new() -> Self {
        Self {
            mb: Mb::new(false),
            placed: Vec::new(),
        }
    }
}

/// Half the width of the body at height `y` (the faces batter in toward the top).
fn half_width(y: f32) -> f32 {
    let t = ((y + SINK) / (BODY + SINK)).clamp(0.0, 1.0);
    FOOT * 0.5 + (TOP - FOOT) * 0.5 * t
}

/// How far the faces lean in per meter of height.
const BATTER: f32 = (FOOT - TOP) * 0.5 / (BODY + SINK);

/// Weathered limestone and gritstone, each stone its own shade; vertex colors tint the neutral
/// stone texture.
const TINTS: [Vec3; 8] = [
    Vec3::new(0.62, 0.6, 0.56),
    Vec3::new(0.52, 0.51, 0.48),
    Vec3::new(0.66, 0.62, 0.54),
    Vec3::new(0.44, 0.43, 0.41),
    Vec3::new(0.58, 0.53, 0.45),
    Vec3::new(0.5, 0.5, 0.5),
    Vec3::new(0.4, 0.37, 0.33),
    Vec3::new(0.6, 0.58, 0.52),
];

/// A stone's color at height `y` above the ground.
fn stone_tint(rng: &mut Rng, y: f32) -> Vec3 {
    let mut c = TINTS[rng.range_i32(0, TINTS.len() as i32) as usize] * rng.range(0.78, 1.1);
    // Lichen: pale gray-green crusts, now and then a yellow-orange one, mostly up top where
    // the stones dry out.
    let high = smoothstep(0.2, BODY, y);
    if rng.chance(0.12 + 0.2 * high) {
        c = c.lerp(Vec3::new(0.74, 0.76, 0.64), rng.range(0.2, 0.5));
    } else if rng.chance(0.03 + 0.06 * high) {
        c = c.lerp(Vec3::new(0.8, 0.6, 0.26), rng.range(0.25, 0.5));
    }
    // Damp, dark and mossy toward the ground.
    let low = 1.0 - smoothstep(-0.1, 0.5, y);
    c.lerp(c * Vec3::new(0.55, 0.68, 0.4), low * rng.range(0.4, 0.95))
}

/// How far inside the faces the core is, m.
const CORE_INSET: f32 = 0.12;
/// The dark core's sides and top.
const DARK: Vec3 = Vec3::new(0.22, 0.21, 0.19);
const DARK_TOP: Vec3 = Vec3::new(0.26, 0.25, 0.22);
/// How far inside the head the dark core of a run's last segment ends, m: deeper than any
/// head stone's outer face wanders, shallower than the shortest one reaches back.
const CORE_UNDER_HEAD: f32 = 0.14;
/// How far short of the +X end an end segment's face stones stop, under the head.
const FACE_UNDER_HEAD: f32 = 0.2;

/// Segment variant `seed`: stones up close, their fronts, and the two cores.
pub fn segment(seed: u64) -> WallSegment {
    let (near, placed) = stones(seed, false);
    let (near_end, placed_end) = stones(seed, true);
    WallSegment {
        near,
        placed,
        near_end,
        placed_end,
        core: core(DARK, DARK_TOP, CoreEnds::Open),
        core_end: core(DARK, DARK_TOP, CoreEnds::UnderHead),
        core_far: core(
            Vec3::new(0.33, 0.32, 0.3),
            Vec3::new(0.4, 0.39, 0.36),
            CoreEnds::Closed,
        ),
    }
}

/// Head variant `seed`.
pub fn head(seed: u64) -> WallHead {
    let (near, placed) = head_stones(seed);
    WallHead { near, placed }
}

/// One weathered stone: `center` and axes `ax` (along, up, out of the face), half extents
/// `half`, shaded for a stone at height `y` in the wall, playing `role`.
#[allow(clippy::too_many_arguments)]
fn stone(
    l: &mut Laying,
    rng: &mut Rng,
    seed: &mut u64,
    center: Vec3,
    ax: [Vec3; 3],
    half: Vec3,
    y: f32,
    role: Role,
) {
    *seed += 1;
    // The hero stone: a dressed block filling the box (a hair bigger, so neighbors touch).
    let size = half * 2.0 * Vec3::new(1.04, 1.04, 1.0);
    l.placed.push(Placed {
        center,
        ax,
        size,
        class: hero_class(role, size),
        variant: (*seed % HERO_VARIANTS as u64) as u8,
        shade: (hash01(*seed as i64, 0, 3, 0x51) * HERO_SHADES as f32) as u8 % HERO_SHADES as u8,
    });
    let mb = &mut l.mb;
    // (The rock texture is pale limestone; the tints darken it toward weathered gritstone.)
    let tint = stone_tint(rng, y) * rng.range(0.58, 0.72);
    // Rocks are rounder than the boxes they fill: a little bigger along the face, so their
    // bellies meet and only chinks show between them.
    let size = half * 2.0 * Vec3::new(1.18, 1.2, 1.0);
    // About 1,300 triangles for small stones, 5,000 for the big ones: facets of a centimeter
    // or two, which virtual geometry thins out with distance.
    let detail = if size.max_element() > 0.6 { 4 } else { 3 };
    // Dress it like a waller would: split along its bedding planes (flat top and bottom) with a
    // squared-off face and ends, so it reads as a slab of gritstone rather than a river cobble.
    // The rock starts a little bigger, so the dressed stone still fills `size`.
    let dressed = Vec3::new(0.95, 0.92, 0.82);
    let mut rock =
        engine::render::rock::rock(size / dressed, (*seed % u32::MAX as u64) as u32, detail);
    let limit = size * 0.5;
    for v in &mut rock.vertices {
        let p = Vec3::from(v.position);
        v.position = [
            soft_clamp(p.x, limit.x, size.x * 0.06),
            soft_clamp(p.y, limit.y, size.y * 0.07),
            soft_clamp(p.z, limit.z, size.z * 0.06),
        ];
    }
    rock.compute_normals();
    let base = mb.mesh.vertices.len() as u32;
    mb.mesh.vertices.extend(rock.vertices.iter().map(|v| {
        let p = Vec3::from(v.position);
        let n = Vec3::from(v.normal);
        let c = v.color;
        let mut out = *v;
        out.position = (center + ax[0] * p.x + ax[1] * p.y + ax[2] * p.z).to_array();
        out.normal = (ax[0] * n.x + ax[1] * n.y + ax[2] * n.z).to_array();
        out.color = [c[0] * tint.x, c[1] * tint.y, c[2] * tint.z, c[3]];
        out
    }));
    mb.mesh
        .indices
        .extend(rock.indices.iter().map(|i| i + base));
}

/// `v` with its magnitude held under `limit`, the corner rounded over a width `k`.
fn soft_clamp(v: f32, limit: f32, k: f32) -> f32 {
    let a = v.abs();
    let h = (0.5 + 0.5 * (limit - a) / k).clamp(0.0, 1.0);
    v.signum() * (limit + (a - limit) * h - k * h * (1.0 - h))
}

/// Both faces' stones in uneven courses, pinning stones, through-stones and the coping (which
/// leaves room for a head at the +X end when `end` is set).
fn stones(seed: u64, end: bool) -> (Mesh, Vec<Placed>) {
    let mut rng = Rng::new(seed);
    let mut l = Laying::new();
    let x0 = -SEGMENT * 0.5;
    let x1 = SEGMENT * 0.5;
    let mut stone_seed = seed * 10_000;
    for side in [-1.0f32, 1.0] {
        // The face's outward normal and its up direction, leaning in with the batter.
        let n = Vec3::new(0.0, BATTER, side).normalize();
        let up = Vec3::new(0.0, 1.0, -side * BATTER).normalize();
        let mut y = -SINK;
        while y < BODY - 0.04 {
            // Big stones at the foot, smaller toward the top.
            let t = ((y + SINK) / (BODY + SINK)).clamp(0.0, 1.0);
            let mut h = rng.range(0.2, 0.32) * (1.0 - 0.45 * t);
            if BODY - (y + h) < 0.08 {
                h = BODY - y;
            }
            // Each course starts somewhere else, so joints don't line up between courses, and
            // its end stones reach a little past the segment's ends, into the neighbors' ends,
            // so there is no straight joint where two segments meet.
            // (Not at an end segment's +X end, which its head closes.)
            let reach = rng.range(0.0, 0.12);
            let (start, stop) = (
                x0 - rng.range(0.0, 0.12),
                if end {
                    x1 - FACE_UNDER_HEAD
                } else {
                    x1 + reach
                },
            );
            let mut x = start - rng.range(0.0, 0.4);
            while x < stop - 0.02 {
                // Long slabs mostly, now and then a squarer block.
                let mut len = if rng.chance(0.25) {
                    h * rng.range(1.0, 1.6)
                } else {
                    rng.range(0.3, 0.85) * (1.0 - 0.4 * t)
                };
                if stop - (x + len) < 0.14 {
                    len = stop - x;
                }
                let (a, b) = (x.max(start), (x + len).min(stop));
                x += len;
                if b - a < 0.07 {
                    continue;
                }
                let gap = rng.range(0.01, 0.03);
                // Stones fill their course unevenly: some short, sitting low or high in it.
                let sh = h * rng.range(0.82, 1.0);
                let yc = y + h * 0.5 + (h - sh) * rng.range(-0.5, 0.5);
                let depth = rng.range(0.2, 0.32);
                // Proud or set back, so the face is not a flat plane.
                let out = rng.range(-0.03, 0.035);
                let surface = Vec3::new((a + b) * 0.5, yc, side * half_width(yc)) + n * out;
                // A rock is widest through its middle: set that a little behind the face, so
                // the stones' full outlines show and they bulge out a few centimeters.
                let center = surface - n * depth * 0.2;
                let tilt = Quat::from_axis_angle(n, rng.range(-0.14, 0.14))
                    * Quat::from_axis_angle(up, rng.range(-0.08, 0.08))
                    * Quat::from_axis_angle(Vec3::X, rng.range(-0.06, 0.06));
                let ax = [tilt * Vec3::X, tilt * up, tilt * n];
                let half = Vec3::new((b - a - gap) * 0.5, (sh - gap * 0.6) * 0.5, depth * 0.5);
                stone(
                    &mut l,
                    &mut rng,
                    &mut stone_seed,
                    center,
                    ax,
                    half,
                    yc,
                    Role::Face,
                );
                // A pinning stone wedged into the joint above a short stone.
                if sh < h * 0.85 && rng.chance(0.5) {
                    let py = yc + sh * 0.5 + (h - sh) * 0.35;
                    let px = rng.range(a + 0.04, b - 0.04);
                    let p = Vec3::new(px, py, side * (half_width(py) - 0.04));
                    let s = (h - sh).clamp(0.04, 0.08);
                    let half = Vec3::new(s * rng.range(0.9, 1.6), s * 0.5, 0.06);
                    stone(
                        &mut l,
                        &mut rng,
                        &mut stone_seed,
                        p,
                        [Vec3::X, up, n],
                        half,
                        py,
                        Role::Face,
                    );
                }
            }
            y += h;
        }
    }
    // The head covers the last stretch of an end segment.
    let top_end = if end { x1 - HEAD_DEPTH + 0.06 } else { x1 };
    // Through-stones: long stones across the whole wall, their ends showing on both faces.
    let mut x = x0 + rng.range(0.4, 0.9);
    while x < top_end - 0.3 {
        let y = rng.range(0.36, 0.5);
        let w = half_width(y) + rng.range(0.03, 0.08);
        let rot = Quat::from_rotation_y(rng.range(-0.12, 0.12));
        let half = Vec3::new(rng.range(0.12, 0.2), rng.range(0.06, 0.09), w);
        stone(
            &mut l,
            &mut rng,
            &mut stone_seed,
            Vec3::new(x, y, 0.0),
            [rot * Vec3::X, Vec3::Y, rot * Vec3::Z],
            half,
            y,
            Role::Through,
        );
        x += rng.range(1.0, 1.7);
    }
    coping(&mut l, &mut rng, &mut stone_seed, x0, top_end);
    (l.mb.mesh, l.placed)
}

/// Coping from `x0` to `x1`: flat stones set on edge across the top, tall and short in turn,
/// leaning a little.
fn coping(l: &mut Laying, rng: &mut Rng, seed: &mut u64, x0: f32, x1: f32) {
    let mut x = x0 + 0.02;
    let mut tall = rng.chance(0.5);
    let lean = rng.range(-0.1, 0.1);
    while x < x1 - 0.05 {
        let t = rng.range(0.06, 0.16).min(x1 - x);
        let h = if tall {
            rng.range(0.24, COPING)
        } else {
            rng.range(0.14, 0.22)
        };
        tall = !tall;
        let w = TOP * 0.5 + rng.range(-0.03, 0.05);
        let rot = Quat::from_rotation_z(lean + rng.range(-0.08, 0.08))
            * Quat::from_rotation_y(rng.range(-0.1, 0.1));
        let center = Vec3::new(x + t * 0.5, BODY + h * 0.5 - 0.04, rng.range(-0.02, 0.02));
        stone(
            l,
            rng,
            seed,
            center,
            [rot * Vec3::X, rot * Vec3::Y, rot * Vec3::Z],
            Vec3::new(t * 0.5 - 0.008, h * 0.5, w),
            BODY + h,
            Role::Coping,
        );
        x += t;
    }
}

/// A head: courses of stones laid across the whole wall at its end, long and short in turn
/// along it so they bond into the wall's faces, with a coping stone on top.
fn head_stones(seed: u64) -> (Mesh, Vec<Placed>) {
    let mut rng = Rng::new(seed);
    let mut l = Laying::new();
    let mut stone_seed = seed * 10_000 + 5_000;
    let mut y = -SINK;
    let mut long = rng.chance(0.5);
    while y < BODY - 0.04 {
        let t = ((y + SINK) / (BODY + SINK)).clamp(0.0, 1.0);
        let mut h = rng.range(0.18, 0.28) * (1.0 - 0.35 * t);
        if BODY - (y + h) < 0.08 {
            h = BODY - y;
        }
        let yc = y + h * 0.5;
        // How far this course's stone reaches back along the wall.
        let reach = if long {
            rng.range(0.42, HEAD_DEPTH)
        } else {
            rng.range(0.24, 0.34)
        };
        long = !long;
        // Its sides are upright, so wide enough to cover the battered faces at its foot.
        let w = half_width(y) + rng.range(0.0, 0.03);
        let rot = Quat::from_rotation_y(rng.range(-0.06, 0.06))
            * Quat::from_rotation_x(rng.range(-0.05, 0.05));
        let center = Vec3::new(-reach * 0.5 + rng.range(-0.01, 0.02), yc, 0.0);
        stone(
            &mut l,
            &mut rng,
            &mut stone_seed,
            center,
            [rot * Vec3::X, rot * Vec3::Y, rot * Vec3::Z],
            Vec3::new(reach * 0.5, h * 0.5 - 0.012, w),
            yc,
            Role::HeadCourse,
        );
        y += h;
    }
    // A big flat stone capping the head.
    let h = rng.range(0.16, 0.22);
    stone(
        &mut l,
        &mut rng,
        &mut stone_seed,
        Vec3::new(-0.24, BODY + h * 0.5 - 0.03, 0.0),
        [Vec3::X, Vec3::Y, Vec3::Z],
        Vec3::new(0.26, h * 0.5, TOP * 0.5 + 0.04),
        BODY + h,
        Role::Cap,
    );
    (l.mb.mesh, l.placed)
}

/// Which ends of a core are closed.
#[derive(Clone, Copy, PartialEq)]
enum CoreEnds {
    /// Neither: inside a run the neighbors close them, and the dark core must not sit in the
    /// plane of a head's outer face.
    Open,
    /// The +X end, pulled back inside the head's stones.
    UnderHead,
    /// Both, for the far core, which is the whole wall.
    Closed,
}

/// The core: a tapered prism just inside the stone faces, from the foot to just under the
/// shortest coping stones' tops, with its sides `side` and its top `top` colored.
fn core(side: Vec3, top: Vec3, ends: CoreEnds) -> Mesh {
    let mut mb = Mb::new(true);
    // Deep enough that the rounded stones, not the core, fill the face.
    let inset = CORE_INSET;
    let y1 = BODY + 0.08;
    let x0 = -SEGMENT * 0.5;
    let x1 = SEGMENT * 0.5
        - if ends == CoreEnds::UnderHead {
            CORE_UNDER_HEAD
        } else {
            0.0
        };
    let hw = |y: f32| half_width(y.min(BODY)) - inset;
    for s in [-1.0f32, 1.0] {
        let n = Vec3::new(0.0, BATTER, s).normalize();
        let pts = [
            Vec3::new(x0, -SINK, s * hw(-SINK)),
            Vec3::new(x1, -SINK, s * hw(-SINK)),
            Vec3::new(x1, y1, s * hw(BODY)),
            Vec3::new(x0, y1, s * hw(BODY)),
        ];
        mb.face(&pts, n, Vec3::X, Vec3::Y, side);
    }
    let lid = [
        Vec3::new(x0, y1, -hw(BODY)),
        Vec3::new(x1, y1, -hw(BODY)),
        Vec3::new(x1, y1, hw(BODY)),
        Vec3::new(x0, y1, hw(BODY)),
    ];
    mb.face(&lid, Vec3::Y, Vec3::X, Vec3::Z, top);
    let closed: &[(f32, f32)] = match ends {
        CoreEnds::Open => &[],
        CoreEnds::UnderHead => &[(x1, 1.0)],
        CoreEnds::Closed => &[(x0, -1.0), (x1, 1.0)],
    };
    for &(x, s) in closed {
        let pts = [
            Vec3::new(x, -SINK, -hw(-SINK)),
            Vec3::new(x, -SINK, hw(-SINK)),
            Vec3::new(x, y1, hw(BODY)),
            Vec3::new(x, y1, -hw(BODY)),
        ];
        mb.face(&pts, Vec3::X * s, Vec3::Z, Vec3::Y, side);
    }
    mb.mesh
}

/// A deterministic 0..1 value for a wall run and a segment along it.
pub fn segment_hash(run: u64, i: u64) -> f32 {
    hash01(run as i64, i as i64, 7, 0x3a11)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tris(m: &Mesh) -> usize {
        m.indices.len() / 3
    }

    #[test]
    fn segments_stay_in_budget_and_inside_their_box() {
        let s = segment(3);
        // Dense enough for virtual geometry (50,000 triangles by default), not wasteful.
        let near = tris(&s.near);
        assert!((60_000..700_000).contains(&near), "near {near}");
        assert!(tris(&s.near_end) < near, "end {}", tris(&s.near_end));
        assert!(tris(&s.core) <= 12, "core {}", tris(&s.core));
        assert!(tris(&s.core_far) <= 12, "far core {}", tris(&s.core_far));
        for v in &s.near.vertices {
            let p = Vec3::from(v.position);
            assert!(p.x.abs() < SEGMENT * 0.5 + 0.3, "{p:?}");
            assert!(p.z.abs() < HALF_FOOT + 0.22, "{p:?}");
            assert!((-SINK - 0.1..HEIGHT + 0.08).contains(&p.y), "{p:?}");
        }
        // An end segment's stones leave its +X end to the head.
        for v in &s.near_end.vertices {
            assert!(v.position[0] < SEGMENT * 0.5 - 0.1, "{:?}", v.position);
        }
    }

    #[test]
    fn heads_cover_the_end_of_the_wall() {
        let h = head(5);
        assert!(tris(&h.near) < 80_000, "head {}", tris(&h.near));
        let (mut min, mut max) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for v in &h.near.vertices {
            let p = Vec3::from(v.position);
            min = min.min(p);
            max = max.max(p);
        }
        // From the outer face back into the wall, across its whole width, foot to coping.
        assert!(max.x > -0.05 && max.x < 0.12, "outer face at {}", max.x);
        assert!(
            min.x < -0.3 && min.x > -HEAD_DEPTH - 0.12,
            "reaches back to {}",
            min.x
        );
        assert!(max.z > half_width(0.0) - 0.02 && min.z < -half_width(0.0) + 0.02);
        assert!(min.y < -SINK + 0.05 && max.y > BODY, "{min:?} {max:?}");
    }

    #[test]
    fn every_stone_has_a_hero_that_fits_its_box() {
        let s = segment(3);
        let h = head(5);
        for (placed, least) in [(&s.placed, 60), (&s.placed_end, 50), (&h.placed, 5)] {
            assert!(placed.len() >= least, "{}", placed.len());
            for p in placed {
                assert!((p.class as usize) < HERO_CLASSES.len());
                assert!((p.variant as usize) < HERO_VARIANTS);
                assert!((p.shade as usize) < HERO_SHADES);
                // Stretched, not distorted beyond recognition.
                let r = p.size / HERO_CLASSES[p.class as usize].1;
                assert!(r.min_element() > 0.25 && r.max_element() < 3.0, "{p:?} {r}");
                // An orthonormal frame (a reflection on one face).
                let m = Mat3::from_cols(p.ax[0], p.ax[1], p.ax[2]);
                assert!((m.determinant().abs() - 1.0).abs() < 1e-3, "{p:?}");
            }
        }
        // A stone per merged rock: about 2,500 triangles each.
        assert!(tris(&s.near) / s.placed.len() > 1000);
    }

    #[test]
    fn the_core_hides_inside_the_stones() {
        // Up close the core shows only through the chinks: its faces are behind the stones'.
        let s = segment(3);
        for core in [&s.core, &s.core_end, &s.core_far] {
            for v in &core.vertices {
                let p = Vec3::from(v.position);
                assert!(
                    p.z.abs() <= half_width(p.y.min(BODY)) - CORE_INSET + 1e-3,
                    "{p:?}"
                );
            }
        }
        // At a run's end the dark core stops inside the head, clear of its outer face.
        for v in &s.core_end.vertices {
            assert!(v.position[0] <= SEGMENT * 0.5 - CORE_UNDER_HEAD + 1e-4);
        }
        // Inside a run, open: no end in the plane where a head could stand.
        assert!(s.core.vertices.iter().all(|v| v.normal[0].abs() < 0.5));
    }
}
