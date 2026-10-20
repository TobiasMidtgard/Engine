//! The Galton board: five hundred steel balls pour from a hopper through twelve rows of brass
//! pins into seventeen bins, and the bins fill in a bell curve.
//!
//! The balls fall in a slot between a glass front and a painted back board only a little wider
//! than a ball, so the board plays out in a plane. Steel bounces: a ball often clears a pin
//! or two before the next one turns it, so the heap spreads wider than a coin tossed at every
//! row would make it (σ = √12 / 2 ≈ 1.7 bins). The curve printed on the back board is the
//! normal curve this board gives, σ ≈ [`SPREAD`] bins, found by running it.
//!
//! What it took (each found by experiment): pins three ball widths apart, as two balls side by
//! side arch over a narrower gap in a plane and jam it; a neck over three balls wide for the
//! same reason; pins set a little off true, as balls dropped straight onto a perfect pin's
//! crown balance there; a plain symmetric pour (a star wheel feeding one ball at a time pushed
//! them all the way it turned); and bins out past the spread, as a wall close to the heap
//! throws balls back in and flattens it.

use std::f32::consts::FRAC_PI_2;

use engine_core::math::{DVec3, Quat, Vec2, Vec3};
use engine_world::physics::{Collider, ColliderShape};
use engine_world::{SurfaceMaterial, SurfaceMaterials};

use super::Yard;
use super::art::{Mat, galton as art};

/// A ball's radius (14 mm bearings), meters.
pub const BALL: f32 = 0.007;
/// How many balls the hopper holds.
pub const BALLS: usize = 500;
/// Rows of pins.
pub const ROWS: usize = 12;
/// Bins.
pub const BINS: usize = 17;
/// Between two pins of a row, and between two bins' middles, meters.
pub const PITCH: f32 = 0.05;
/// A pin's radius, meters.
pub const PIN: f32 = 0.004;
/// How far each pin is set off true, at most, meters.
const PIN_SLOP: f32 = 0.0015;
/// The spread of the heap, bins (one standard deviation).
pub const SPREAD: f32 = 3.2;
/// The slot between the glass and the back board: a little wider than a ball.
pub const SLOT: f32 = 2.0 * BALL + 0.001;
/// Half the width inside the board, meters: the bins' outer walls.
pub const HALF_WIDTH: f32 = PITCH * (BINS as f32) * 0.5;
/// The bins' floor above the ground, meters.
pub const FLOOR: f32 = 0.35;
/// The tops of the bins' dividers, meters.
pub const BIN_TOP: f32 = 0.7;
/// The lowest row of pins, meters.
const LOW_ROW: f32 = BIN_TOP + 0.05;
/// Between two rows of pins (the pins sit in equilateral triangles), meters.
const ROW_STEP: f32 = PITCH * 0.866_025_4;
/// Half the funnel's neck (3.2 balls wide), and its walls' slope from the horizontal, radians.
const NECK: f32 = 3.2 * BALL;
const FUNNEL_SLOPE: f32 = 0.96;
/// The neck's height above the top row of pins, meters.
const NECK_GAP: f32 = 0.09;
/// Wall and divider thickness: walls, and the thin aluminium dividers.
const WALL: f32 = 0.02;
const DIVIDER: f32 = 0.004;
/// The glass and the back board's thickness.
const PANE: f32 = 0.008;

/// The middle of row `k` of pins (0 the top row), meters above the ground.
pub fn row_height(k: usize) -> f32 {
    LOW_ROW + (ROWS - 1 - k) as f32 * ROW_STEP
}

/// The pins, as `(x, y)` in the board's plane: rows right across the board, each shifted half
/// a pitch from the one above, the top one with a pin under the neck and the bottom one with
/// its gaps over the bins' middles. Each is set by hand, up to [`PIN_SLOP`] out.
pub fn pins() -> Vec<(f32, f32)> {
    let reach = (BINS / 2) as f32 * PITCH;
    let mut rng = super::Rng(0x9a17);
    let mut out = Vec::new();
    for k in 0..ROWS {
        let shift = if k % 2 == 0 { 0.0 } else { 0.5 };
        for i in -(BINS as i32)..=BINS as i32 {
            let x = (i as f32 + shift) * PITCH;
            if x.abs() <= reach + 1e-4 {
                let dx = (rng.next() - 0.5) * 2.0 * PIN_SLOP;
                let dy = (rng.next() - 0.5) * 2.0 * PIN_SLOP;
                out.push((x + dx, row_height(k) + dy));
            }
        }
    }
    out
}

/// The bin a ball at `x` (across the board) lies in.
pub fn bin_of(x: f32) -> usize {
    ((x + HALF_WIDTH) / PITCH)
        .floor()
        .clamp(0.0, (BINS - 1) as f32) as usize
}

/// The normal curve's height `x` bins from the middle: the share of the balls a bin there
/// holds.
pub fn share(x: f32) -> f32 {
    (-x * x / (2.0 * SPREAD * SPREAD)).exp() / (SPREAD * std::f32::consts::TAU.sqrt())
}

/// How many balls each bin should hold, on the printed curve.
pub fn expected() -> [f32; BINS] {
    let mid = (BINS / 2) as f32;
    std::array::from_fn(|j| BALLS as f32 * share(j as f32 - mid))
}

/// A board whose top face runs from `from` to `to` in the board's plane, lying to the right
/// of that line as you walk it, `thick` deep and spanning the slot: (center, half extents,
/// turn about Z).
fn slab(from: Vec2, to: Vec2, thick: f32) -> (Vec3, Vec3, f32) {
    let along = to - from;
    let angle = along.y.atan2(along.x);
    let down = Vec2::new(angle.sin(), -angle.cos()) * thick * 0.5;
    let c = (from + to) * 0.5 + down;
    (
        Vec3::new(c.x, c.y, 0.0),
        Vec3::new(along.length() * 0.5, thick * 0.5, SLOT * 0.5),
        angle,
    )
}

/// The neck's height above the ground, meters.
pub fn neck() -> f32 {
    row_height(0) + NECK_GAP
}

/// The funnel's two walls, from the neck up into the sides (so no ball slips down past a
/// wall's top end), as (center, half extents, turn about Z).
pub fn guides() -> Vec<(Vec3, Vec3, f32)> {
    let reach = HALF_WIDTH + WALL;
    [-1.0f32, 1.0]
        .into_iter()
        .map(|s| {
            let lip = Vec2::new(s * NECK, neck());
            let side = Vec2::new(s * reach, funnel_top());
            if s > 0.0 {
                slab(lip, side, WALL)
            } else {
                slab(side, lip, WALL)
            }
        })
        .collect()
}

/// Where the funnel's walls meet the sides, meters above the ground.
fn funnel_top() -> f32 {
    neck() + (HALF_WIDTH + WALL - NECK) * FUNNEL_SLOPE.tan()
}

/// The board's top, meters: a little above the heap of balls in the hopper.
pub fn top() -> f32 {
    hopper().iter().map(|p| p.1).fold(0.0, f32::max) + BALL + 0.03
}

/// Where the balls start: packed in rows above the funnel, as `(x, y)` in the board's plane.
pub fn hopper() -> Vec<(f32, f32)> {
    let pitch = 2.0 * BALL + 0.0015;
    let across = ((2.0 * (HALF_WIDTH - BALL)) / pitch).floor() as usize;
    let mut out = Vec::with_capacity(BALLS);
    // Up to half a millimeter off the lattice each, so the heap doesn't settle in lockstep.
    let mut rng = super::Rng(0x6a17);
    let mut row = 0;
    while out.len() < BALLS {
        let shift = if row % 2 == 0 { 0.0 } else { pitch * 0.5 };
        let y = funnel_top() + BALL + 0.01 + row as f32 * pitch * 0.866;
        for i in 0..across - row % 2 {
            if out.len() == BALLS {
                break;
            }
            let x = -HALF_WIDTH + BALL + shift + i as f32 * pitch;
            out.push((x + (rng.next() - 0.5) * 0.001, y));
        }
        row += 1;
    }
    out
}

/// The fixed boxes: (center, half extents) in the board's frame (x across, y up, z out of
/// the glass), without the funnel's slanted walls and the pins.
pub fn walls() -> Vec<(Vec3, Vec3)> {
    let mut out = Vec::new();
    let h = SLOT * 0.5;
    let mid = (FLOOR + top()) * 0.5;
    let tall = (top() - FLOOR) * 0.5;
    // The bins' floor, and the sides from it to the top.
    out.push((
        Vec3::new(0.0, FLOOR - WALL * 0.5, 0.0),
        Vec3::new(HALF_WIDTH + WALL, WALL * 0.5, h + PANE),
    ));
    for s in [-1.0f32, 1.0] {
        out.push((
            Vec3::new(s * (HALF_WIDTH + WALL * 0.5), mid, 0.0),
            Vec3::new(WALL * 0.5, tall, h + PANE),
        ));
    }
    // The dividers between the bins.
    for j in 1..BINS {
        let x = -HALF_WIDTH + j as f32 * PITCH;
        out.push((
            Vec3::new(x, (FLOOR + BIN_TOP) * 0.5, 0.0),
            Vec3::new(DIVIDER * 0.5, (BIN_TOP - FLOOR) * 0.5, h),
        ));
    }
    out
}

/// The glass in front and the back board behind: (center, half extents).
pub fn panes() -> [(Vec3, Vec3); 2] {
    let h = SLOT * 0.5;
    let mid = (FLOOR + top()) * 0.5;
    let half = Vec3::new(HALF_WIDTH + WALL, (top() - FLOOR) * 0.5 + WALL, PANE * 0.5);
    [
        (Vec3::new(0.0, mid, h + PANE * 0.5), half),
        (Vec3::new(0.0, mid, -h - PANE * 0.5), half),
    ]
}

/// The Galton board standing on the yard at `base`, its glass facing +Z, the hopper full.
pub(super) fn galton_board(y: &mut Yard, base: DVec3) {
    let steel = surface(y, "bearing_steel", || SurfaceMaterial {
        name: "bearing_steel".into(),
        friction: 0.25,
        restitution: 0.5,
        density: 7800.0,
        footstep_sound: "footstep.metal".into(),
        impact_effect: "impact.metal".into(),
        ..Default::default()
    });
    let glass = surface(y, "float_glass", || SurfaceMaterial {
        name: "float_glass".into(),
        friction: 0.1,
        restitution: 0.6,
        density: 2500.0,
        footstep_sound: "footstep.glass".into(),
        impact_effect: "impact.glass".into(),
        ..Default::default()
    });
    let brass = surface(y, "pin_brass", || SurfaceMaterial {
        name: "pin_brass".into(),
        friction: 0.25,
        restitution: 0.5,
        density: 8500.0,
        footstep_sound: "footstep.metal".into(),
        impact_effect: "impact.metal".into(),
        ..Default::default()
    });
    let wood = y.wood;
    for (at, half) in walls() {
        y.fixed(
            base + at.as_dvec3(),
            Quat::IDENTITY,
            ColliderShape::Cuboid { half_extents: half },
            wood,
        );
    }
    let [front, back] = panes();
    for ((at, half), s) in [(front, glass), (back, wood)] {
        y.fixed(
            base + at.as_dvec3(),
            Quat::IDENTITY,
            ColliderShape::Cuboid { half_extents: half },
            s,
        );
    }
    for (at, half, turn) in guides() {
        y.fixed(
            base + at.as_dvec3(),
            Quat::from_rotation_z(turn),
            ColliderShape::Cuboid { half_extents: half },
            steel,
        );
    }
    // Pins along Z, across the slot.
    let pin = ColliderShape::Cylinder {
        radius: PIN,
        half_height: SLOT * 0.5,
    };
    for (x, py) in pins() {
        y.fixed(
            base + DVec3::new(x as f64, py as f64, 0.0),
            Quat::from_rotation_x(FRAC_PI_2),
            pin.clone(),
            brass,
        );
    }
    let board = y.parts(
        "galton_board",
        [
            Mat::Lacquer(art::WALNUT),
            Mat::Paint,
            Mat::Brass,
            Mat::Chrome,
            Mat::Glass,
            Mat::Lacquer(art::CURVE),
        ],
        || {
            let b = art::board();
            [b.wood, b.board, b.pins, b.dividers, b.glass, b.curve]
        },
    );
    y.scenery(base, Quat::IDENTITY, board);
    for (i, (x, by)) in hopper().into_iter().enumerate() {
        let parts = y.parts("galton_ball", [Mat::Chrome], || [art::ball(BALL)]);
        let c = Collider::sphere(BALL)
            .with_surface(steel)
            .with_density(7800.0);
        // Turned a little each, so their reflections don't line up.
        let turn = Quat::from_rotation_y(i as f32 * 0.37);
        let ball = y.body(base + DVec3::new(x as f64, by as f64, 0.0), turn, c, parts);
        y.name(ball, "Galton ball");
    }
}

fn surface(
    y: &mut Yard,
    name: &str,
    make: impl FnOnce() -> SurfaceMaterial,
) -> engine_world::SurfaceId {
    let mut table = y.world.get_resource_or_init::<SurfaceMaterials>();
    match table.id(name) {
        Some(id) => id,
        None => table.add(make()),
    }
}
