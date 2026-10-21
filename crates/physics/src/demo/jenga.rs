//! Giant Jenga: fifty-four oak blocks, eighteen levels of three, each level across the one
//! under it. A test rig's steel rod pokes the loose middle block out of three levels, one after
//! another, and the tower stands on; then it pushes out a block the tower rests on, and the top
//! half tips over and comes down.
//!
//! The blocks are plain rigid bodies. As in a real set, not every block is the same thickness:
//! the middle ones of the poked levels are 3 mm thinner, so nothing above rests on them and
//! they slide out under a push. The rod is a kinematic body moved along its path through its
//! `Transform` each step ([`drive_rods`]); it glides there over the step, shoving what it meets.

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_world::physics::{Collider, ColliderShape, RigidBody};

use super::art::{Mat, jenga as art};
use super::{ProvingGround, Station, StationPart, Yard};

/// A block's length, width and thickness, meters (a giant garden set: three widths make a
/// length, and the tower stands 2.7 m).
pub const LONG: f32 = 0.75;
pub const WIDE: f32 = 0.25;
pub const HIGH: f32 = 0.15;
/// Levels of three.
pub const LEVELS: usize = 18;
/// How much thinner the loose blocks are, meters.
pub const LOOSE: f32 = 0.003;
/// Oak, kg/m³ (a block weighs 17 kg).
pub const DENSITY: f32 = 600.0;

/// What the rod pokes out, in order: (level, slot across it: -1, 0 or 1). Levels count from 0
/// at the bottom; even levels lie along X, so the rod pushes along X from the -X side. The
/// first three are loose middle blocks; the last carries level 8 once its middle is gone.
pub const POKES: [(usize, i32); 4] = [(4, 0), (8, 0), (12, 0), (8, 1)];

/// The rod: radius and length, meters.
pub const ROD: f32 = 0.03;
pub const ROD_LONG: f32 = 1.6;
/// The rod's tip waits this far along X from the tower's axis before a poke (outside the
/// tower's face at -LONG / 2), and pushes on to here: the block's inner end clears the far
/// face and it drops off. (Pushed only until its middle hangs out, it tips, its inner end
/// jams up against the level above, and it stays there, sticking out.)
pub const TIP_OUT: f32 = -0.6;
pub const TIP_IN: f32 = 0.45;
/// The tip's parking spot, out of the way before the first poke and after the last.
pub const PARK: Vec3 = Vec3::new(-1.0, 1.35, 0.0);
/// How fast the rod pushes, m/s.
pub const PUSH_SPEED: f32 = 0.35;
/// When the first poke starts and how far apart they start, seconds.
pub const FIRST_POKE: f32 = 1.5;
pub const POKE_EVERY: f32 = 5.5;
/// Seconds to line up with the next block and to pull back out of the tower.
const LINE_UP: f32 = 1.2;
const PULL_BACK: f32 = 1.4;

/// A level's middle height above the slab, meters.
pub fn level_y(level: usize) -> f32 {
    HIGH * (level as f32 + 0.5)
}

/// Is this block one of the loose (thinner) ones?
pub fn loose(level: usize, slot: i32) -> bool {
    slot == 0 && POKES[..3].contains(&(level, slot))
}

/// A block's center and half extents in the station's frame (origin on the slab under the
/// tower's axis): even levels lie along X, odd ones along Z. Loose blocks sit on the level
/// below like the rest, with the gap above them.
pub fn block(level: usize, slot: i32) -> (Vec3, Vec3) {
    let high = if loose(level, slot) {
        HIGH - LOOSE
    } else {
        HIGH
    };
    let y = HIGH * level as f32 + high * 0.5;
    let across = slot as f32 * WIDE;
    if level.is_multiple_of(2) {
        (
            Vec3::new(0.0, y, across),
            Vec3::new(LONG * 0.5, high * 0.5, WIDE * 0.5),
        )
    } else {
        (
            Vec3::new(across, y, 0.0),
            Vec3::new(WIDE * 0.5, high * 0.5, LONG * 0.5),
        )
    }
}

/// The rod tip's path: (seconds into the station's cycle, position in the station's frame).
pub fn path() -> Vec<(f32, Vec3)> {
    let mut keys = vec![(0.0, PARK)];
    for (i, &(level, slot)) in POKES.iter().enumerate() {
        let start = FIRST_POKE + i as f32 * POKE_EVERY;
        let last = keys.last().unwrap().1;
        let out = Vec3::new(TIP_OUT, level_y(level), slot as f32 * WIDE);
        let pushed = (TIP_IN - TIP_OUT) / PUSH_SPEED;
        keys.push((start, last));
        keys.push((start + LINE_UP, out));
        keys.push((start + LINE_UP + pushed, Vec3::new(TIP_IN, out.y, out.z)));
        keys.push((start + LINE_UP + pushed + PULL_BACK, out));
    }
    let end = keys.last().unwrap().0;
    keys.push((end + 1.0, PARK));
    keys
}

/// Where the rod's tip is `secs` into the cycle (it holds still between the path's keys).
pub fn tip(secs: f32) -> Vec3 {
    let keys = path();
    match keys.iter().position(|(t, _)| *t > secs) {
        None => keys.last().unwrap().1,
        Some(0) => keys[0].1,
        Some(i) => {
            let ((t0, a), (t1, b)) = (keys[i - 1], keys[i]);
            a.lerp(b, (secs - t0) / (t1 - t0))
        }
    }
}

/// When the last poke starts, seconds into the cycle.
pub fn last_poke() -> f32 {
    FIRST_POKE + (POKES.len() - 1) as f32 * POKE_EVERY
}

/// The rod lies along X with its tip at +X; its body's center is half a rod behind the tip.
fn rod_center(secs: f32) -> Vec3 {
    tip(secs) - Vec3::X * (ROD_LONG * 0.5)
}

/// The rod's body (yard-local station base), moved along [`tip`] by [`drive_rods`].
#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct JengaRod {
    base: DVec3,
}

/// The test rig's frame: two posts behind the rod's carriage and a beam across their tops,
/// in the station's frame: (center, half extents).
pub fn rig_frame() -> Vec<(Vec3, Vec3)> {
    let x = PARK.x - ROD_LONG - 0.25;
    let (post, high, apart) = (0.07, 3.0, 0.75);
    vec![
        (
            Vec3::new(x, high * 0.5, -apart),
            Vec3::new(post, high * 0.5, post),
        ),
        (
            Vec3::new(x, high * 0.5, apart),
            Vec3::new(post, high * 0.5, post),
        ),
        (
            Vec3::new(x, high + post, 0.0),
            Vec3::new(post, post, apart + post),
        ),
        (Vec3::new(x, 0.03, 0.0), Vec3::new(0.3, 0.03, apart + 0.3)),
    ]
}

pub(super) fn jenga(y: &mut Yard, base: DVec3) {
    for level in 0..LEVELS {
        for slot in [-1, 0, 1] {
            let (at, half) = block(level, slot);
            let c = Collider::cuboid(half)
                .with_surface(y.wood)
                .with_density(DENSITY);
            let seed = (level * 3 + (slot + 1) as usize) as u32;
            let key = format!(
                "jenga_block/{:.4}x{:.4}x{:.4}/{}",
                half.x,
                half.y,
                half.z,
                seed % 4
            );
            let parts = y.parts(&key, [Mat::Oak], || [art::block(half, seed % 4)]);
            let e = y.body(base + at.as_dvec3(), Quat::IDENTITY, c, parts);
            y.name(e, &format!("Jenga block {level} {slot}"));
        }
    }

    for (at, half) in rig_frame() {
        y.fixed(
            base + at.as_dvec3(),
            Quat::IDENTITY,
            ColliderShape::Cuboid { half_extents: half },
            y.metal,
        );
    }
    let frame = y.parts("jenga_rig", [Mat::Yellow], || [art::rig()]);
    y.scenery(base, Quat::IDENTITY, frame);

    // The rod: a capsule along its body's Y, turned to lie along X.
    let along_x = Quat::from_rotation_z(-std::f32::consts::FRAC_PI_2);
    let parts = y.parts("jenga_rod", [Mat::Steel, Mat::Yellow], || {
        let r = art::rod();
        [r.steel, r.carriage]
    });
    let rod = y.spawn((
        Transform::from_translation(y.at(base + rod_center(0.0).as_dvec3())).with_rotation(along_x),
        RigidBody::kinematic(),
        Collider::capsule(ROD, ROD_LONG * 0.5 - ROD).with_surface(y.metal),
        JengaRod { base },
        Name::new("Jenga rod"),
    ));
    y.dress(rod, parts);
}

/// Move each Jenga rod to where its path has it this far into its station's cycle.
pub(crate) fn drive_rods(
    grounds: Query<&ProvingGround>,
    mut rods: Query<(&JengaRod, &StationPart, &mut Transform)>,
) {
    for (rod, part, mut t) in &mut rods {
        let Ok(g) = grounds.get(part.ground) else {
            continue;
        };
        let secs = g.running[Station::Jenga as usize] as f32;
        let at = g.origin + rod.base + rod_center(secs).as_dvec3();
        if t.translation != at {
            t.translation = at;
        }
    }
}
