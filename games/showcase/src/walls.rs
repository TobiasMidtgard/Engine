//! Dry-stone walls between the fields of the valley floor: rough field stones laid dry in
//! uneven courses that lean in toward the top, big stones at the foot and smaller ones above,
//! little pinning stones wedged into the gaps, through-stones tying the two faces together, a
//! dark rubble core showing in the chinks and a coping of stones set on edge. A run of wall
//! ends in a head: stones laid across the whole wall, so the end is built, not cut.
//!
//! A wall is a row of straight segments. Every segment is one of a few variants, each built as
//! meshes the scenery shares: the stones up close (chamfered, rough-hewn), only their front
//! faces a little further off, and the core, a tapered prism just inside the stones: dark up
//! close (the shadowed rubble between the stones), stone-colored from afar, where it is the
//! whole wall.
//!
//! Build frame: the segment runs along X from `-SEGMENT / 2` to `SEGMENT / 2`, centered on
//! Z = 0, with the ground at y = 0 (the foot goes [`SINK`] meters below it). A head is built
//! with its outer face at X = 0, the wall it ends running off toward -X.

use crate::village::Mb;
use engine::core::math::{hash01, smoothstep};
use engine::prelude::*;

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
    /// Chamfered, rough-hewn stones.
    pub near: Mesh,
    /// The same stones' front faces.
    pub mid: Mesh,
    /// The last segment of a run: its coping stops short of the +X end, where the head goes.
    pub near_end: Mesh,
    pub mid_end: Mesh,
    /// The core between the faces: dark, for behind the stones. Open at its ends, which the
    /// neighbors close.
    pub core: Mesh,
    /// The dark core of a run's last segment: its +X end stops inside the head's stones.
    pub core_end: Mesh,
    /// The core stone-colored, the whole wall from afar.
    pub core_far: Mesh,
}

/// A wall head's meshes: stones up close and their outer faces further off.
pub struct WallHead {
    pub near: Mesh,
    pub mid: Mesh,
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
    WallSegment {
        near: stones(seed, false, false),
        mid: stones(seed, true, false),
        near_end: stones(seed, false, true),
        mid_end: stones(seed, true, true),
        core: core(DARK, DARK_TOP, CoreEnds::Open),
        core_end: core(DARK, DARK_TOP, CoreEnds::UnderHead),
        core_far: core(
            Vec3::new(0.42, 0.41, 0.38),
            Vec3::new(0.52, 0.5, 0.46),
            CoreEnds::Closed,
        ),
    }
}

/// Head variant `seed`.
pub fn head(seed: u64) -> WallHead {
    WallHead {
        near: head_stones(seed, false),
        mid: head_stones(seed, true),
    }
}

/// One rough stone: `center` and axes `ax` (along, up, out of the face), half extents `half`.
/// `far` builds draw the same random numbers as near ones, so both show the same stones.
#[allow(clippy::too_many_arguments)]
fn stone(
    mb: &mut Mb,
    rng: &mut Rng,
    seed: &mut u64,
    far: bool,
    center: Vec3,
    ax: [Vec3; 3],
    half: Vec3,
    y: f32,
    skip: u8,
) {
    *seed += 1;
    // Field stones: well rounded, lumpy.
    let c = rng.range(0.03, 0.07);
    let tint = stone_tint(rng, y);
    let rough = rng.range(0.02, 0.045);
    // Without chamfers to bridge them, rough faces would open cracks at the stone's edges;
    // from further off the lumps don't show anyway.
    let rough = if far { 0.0 } else { rough };
    mb.cbox_rough(center, ax, half, c, tint, skip, rough, *seed);
}

/// Both faces' stones in uneven courses, pinning stones, through-stones and the coping (which
/// leaves room for a head at the +X end when `end` is set).
fn stones(seed: u64, far: bool, end: bool) -> Mesh {
    let mut rng = Rng::new(seed);
    let mut mb = Mb::new(far);
    let x0 = -SEGMENT * 0.5;
    let x1 = SEGMENT * 0.5;
    let mut stone_seed = seed * 10_000;
    // The back of a face stone is inside the wall; from further off only the fronts show.
    let face_skip = if far { 0b01_1111 } else { 1 << 4 };
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
                let center = surface - n * depth * 0.5;
                let tilt = Quat::from_axis_angle(n, rng.range(-0.14, 0.14))
                    * Quat::from_axis_angle(up, rng.range(-0.08, 0.08))
                    * Quat::from_axis_angle(Vec3::X, rng.range(-0.06, 0.06));
                let ax = [tilt * Vec3::X, tilt * up, tilt * n];
                let half = Vec3::new((b - a - gap) * 0.5, (sh - gap * 0.6) * 0.5, depth * 0.5);
                stone(
                    &mut mb,
                    &mut rng,
                    &mut stone_seed,
                    far,
                    center,
                    ax,
                    half,
                    yc,
                    face_skip,
                );
                // A pinning stone wedged into the joint above a short stone (drawn in far
                // builds too, so the stones after it stay the same; too small to show there).
                if sh < h * 0.85 && rng.chance(0.5) {
                    let py = yc + sh * 0.5 + (h - sh) * 0.35;
                    let px = rng.range(a + 0.04, b - 0.04);
                    let p = Vec3::new(px, py, side * (half_width(py) - 0.04));
                    let s = (h - sh).clamp(0.04, 0.08);
                    let half = Vec3::new(s * rng.range(0.9, 1.6), s * 0.5, 0.06);
                    let mut skip = face_skip;
                    if far {
                        // Every side skipped: the draws happen, nothing is built.
                        skip = 0b11_1111;
                    }
                    stone(
                        &mut mb,
                        &mut rng,
                        &mut stone_seed,
                        far,
                        p,
                        [Vec3::X, up, n],
                        half,
                        py,
                        skip,
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
            &mut mb,
            &mut rng,
            &mut stone_seed,
            far,
            Vec3::new(x, y, 0.0),
            [rot * Vec3::X, Vec3::Y, rot * Vec3::Z],
            half,
            y,
            if far { 0b00_1111 } else { 0 },
        );
        x += rng.range(1.0, 1.7);
    }
    coping(&mut mb, &mut rng, &mut stone_seed, x0, top_end, far);
    mb.mesh
}

/// Coping from `x0` to `x1`: flat stones set on edge across the top, tall and short in turn,
/// leaning a little.
fn coping(mb: &mut Mb, rng: &mut Rng, seed: &mut u64, x0: f32, x1: f32, far: bool) {
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
            mb,
            rng,
            seed,
            far,
            center,
            [rot * Vec3::X, rot * Vec3::Y, rot * Vec3::Z],
            Vec3::new(t * 0.5 - 0.008, h * 0.5, w),
            BODY + h,
            // The bottom sits on the wall; from further off the ends against the neighbors
            // don't show either.
            if far { 0b00_0111 } else { 1 << 2 },
        );
        x += t;
    }
}

/// A head: courses of stones laid across the whole wall at its end, long and short in turn
/// along it so they bond into the wall's faces, with a coping stone on top.
fn head_stones(seed: u64, far: bool) -> Mesh {
    let mut rng = Rng::new(seed);
    let mut mb = Mb::new(far);
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
            &mut mb,
            &mut rng,
            &mut stone_seed,
            far,
            center,
            [rot * Vec3::X, rot * Vec3::Y, rot * Vec3::Z],
            Vec3::new(reach * 0.5, h * 0.5 - 0.012, w),
            yc,
            // From further off: the outer end and the two faces.
            if far { 0b00_1101 } else { 0 },
        );
        y += h;
    }
    // A big flat stone capping the head.
    let h = rng.range(0.16, 0.22);
    stone(
        &mut mb,
        &mut rng,
        &mut stone_seed,
        far,
        Vec3::new(-0.24, BODY + h * 0.5 - 0.03, 0.0),
        [Vec3::X, Vec3::Y, Vec3::Z],
        Vec3::new(0.26, h * 0.5, TOP * 0.5 + 0.04),
        BODY + h,
        if far { 0b00_0101 } else { 1 << 2 },
    );
    mb.mesh
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
    let inset = 0.05;
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
        assert!(tris(&s.near) < 14_000, "near {}", tris(&s.near));
        assert!(
            tris(&s.near_end) < tris(&s.near),
            "end {}",
            tris(&s.near_end)
        );
        assert!(tris(&s.mid) < tris(&s.near) / 6, "mid {}", tris(&s.mid));
        assert!(tris(&s.core) <= 12, "core {}", tris(&s.core));
        assert!(tris(&s.core_far) <= 12, "far core {}", tris(&s.core_far));
        for v in &s.near.vertices {
            let p = Vec3::from(v.position);
            assert!(p.x.abs() < SEGMENT * 0.5 + 0.25, "{p:?}");
            assert!(p.z.abs() < HALF_FOOT + 0.14, "{p:?}");
            assert!((-SINK - 0.1..HEIGHT + 0.08).contains(&p.y), "{p:?}");
        }
        // An end segment's stones leave its +X end to the head.
        for v in &s.near_end.vertices {
            assert!(v.position[0] < SEGMENT * 0.5 - 0.1, "{:?}", v.position);
        }
    }

    /// The highest point of a mesh in each 0.25 m slice along the wall.
    fn top_profile(m: &Mesh) -> Vec<f32> {
        let mut top = vec![f32::MIN; (SEGMENT / 0.25) as usize + 4];
        for v in &m.vertices {
            let i =
                ((v.position[0] + SEGMENT * 0.5) / 0.25 + 2.0).clamp(0.0, top.len() as f32 - 1.0);
            top[i as usize] = top[i as usize].max(v.position[1]);
        }
        top
    }

    #[test]
    fn the_lods_show_the_same_stones() {
        // At the near-to-mid swap the coping must not jump: both are built from one stream.
        for seed in [900, 901, 902, 903] {
            let s = segment(seed);
            for (near, mid) in [(&s.near, &s.mid), (&s.near_end, &s.mid_end)] {
                let (pn, pm) = (top_profile(near), top_profile(mid));
                // Inside the segment: at its ends a stone's leaning corner may or may not
                // reach into the slice.
                for (i, (a, b)) in pn.iter().zip(pm).enumerate().take(pn.len() - 4).skip(4) {
                    if *a > f32::MIN && b > f32::MIN {
                        assert!((a - b).abs() < 0.08, "seed {seed} slice {i}: {a} vs {b}");
                    }
                }
            }
        }
    }

    #[test]
    fn heads_cover_the_end_of_the_wall() {
        let h = head(5);
        assert!(tris(&h.near) < 2_000, "head {}", tris(&h.near));
        assert!(tris(&h.mid) < tris(&h.near), "head mid {}", tris(&h.mid));
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
    fn the_core_hides_inside_the_stones() {
        // Up close the core shows only through the chinks: its faces are behind the stones'.
        let s = segment(3);
        for core in [&s.core, &s.core_end, &s.core_far] {
            for v in &core.vertices {
                let p = Vec3::from(v.position);
                assert!(p.z.abs() <= half_width(p.y.min(BODY)) - 0.049, "{p:?}");
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
