//! The climbing quarter: a small block of old-town houses north of the movement course to try
//! the climbing on (Assassin's Creed Unity's Paris in miniature, with Titanfall's lines across
//! it).
//!
//! Seven buildings, two to five storeys (7 to 17.5 m), along an east–west street with an
//! alley north between the first two:
//!
//! - **Facades** of plaster with a stone band at every floor and a sill under every window:
//!   a handhold every 1.75 m all the way up, so holding toward a wall climbs it ledge by
//!   ledge, crouch drops back down, jump along it leaps across. Cornices at the roofs.
//! - **Roofs** flat, with chimneys, 2 to 4 m apart to jump across (the street is 6 m: a
//!   plank bridge crosses it between two roofs).
//! - **The alley**, 4 m wide, to wall-run and wall-jump up between the walls.
//! - **Ropes**: from a hoist beam on the tall townhouse, from the bell tower's bracket, and
//!   from a gallows frame on the little square to swing on.
//! - **A zip line** from the bell tower's top over the street to the square.
//! - **Low starts**: crates, a cart and a well to vault and climb from.
//!
//! Collision is the exact boxes of [`pieces`] ([`Piece::collides`]); [`crate::look::city`]
//! draws them and the dressing (windows, shutters, roofs, cobbles).

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_world::physics::{Collider, RigidBody};

use crate::climbrope::ClimbRope;
use crate::zipline::Zipline;

/// The name on every box of the quarter.
pub const NAME: &str = "Climbing quarter";

/// The quarter's origin, from the movement demo's origin (north of the course).
pub const OFFSET: [f64; 3] = [0.0, 0.0, 62.0];
/// Height of a storey, m: a band every storey and a sill half way, so a handhold every
/// half storey.
pub const STOREY: f64 = 3.5;
/// How far bands and sills stand out of the face, m.
pub const BAND_DEPTH: f32 = 0.3;
/// Band and sill heights (tall), m.
pub const BAND_HEIGHT: f32 = 0.25;
pub const SILL_HEIGHT: f32 = 0.2;
/// A window's width and its spacing along a facade, m.
pub const WINDOW_WIDTH: f64 = 1.2;
pub const WINDOW_SPACING: f64 = 3.0;

/// What a piece is made of (the look draws each kind with its material).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// A building's body, in one of three plaster colors.
    Plaster(u8),
    /// Bands, sills, cornices, the tower's stone, the well.
    Stone,
    /// The street and square.
    Cobbles,
    /// Roof caps.
    Zinc,
    /// Beams, posts, crates, the cart, the bridge's planks.
    Timber,
}

/// A box of the quarter: center (from the quarter's origin) and full size, m.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Piece {
    pub center: [f64; 3],
    pub size: [f32; 3],
    pub kind: Kind,
}

impl Piece {
    fn new(min: [f64; 3], max: [f64; 3], kind: Kind) -> Self {
        Self {
            center: std::array::from_fn(|i| (min[i] + max[i]) * 0.5),
            size: std::array::from_fn(|i| (max[i] - min[i]) as f32),
            kind,
        }
    }

    /// Characters collide with it (everything but the roof caps, drawn on the bodies' tops).
    pub fn collides(&self) -> bool {
        self.kind != Kind::Zinc
    }

    pub fn min(&self) -> DVec3 {
        DVec3::from_array(self.center) - Vec3::from(self.size).as_dvec3() * 0.5
    }

    pub fn max(&self) -> DVec3 {
        DVec3::from_array(self.center) + Vec3::from(self.size).as_dvec3() * 0.5
    }
}

/// A building: footprint (x and z, from the quarter's origin), storeys, plaster color, and
/// which faces carry windows (and so sills): −X, +X, −Z, +Z.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Building {
    pub name: &'static str,
    pub x: [f64; 2],
    pub z: [f64; 2],
    pub storeys: u32,
    pub plaster: u8,
    pub windows: [bool; 4],
}

impl Building {
    pub fn height(&self) -> f64 {
        self.storeys as f64 * STOREY
    }
}

/// The houses. The street runs along X at z −3..3, the alley north at x −2..2.
pub const BUILDINGS: [Building; 7] = [
    Building {
        name: "Bakery",
        x: [-22.0, -2.0],
        z: [3.0, 13.0],
        storeys: 3,
        plaster: 0,
        windows: [true, true, true, false],
    },
    Building {
        name: "Townhouse",
        x: [2.0, 11.0],
        z: [3.0, 13.0],
        storeys: 4,
        plaster: 1,
        windows: [true, true, true, false],
    },
    Building {
        name: "Warehouse",
        x: [13.5, 24.0],
        z: [3.0, 13.0],
        storeys: 2,
        plaster: 2,
        windows: [true, false, true, false],
    },
    Building {
        name: "Inn",
        x: [-24.0, -13.5],
        z: [-13.0, -3.0],
        storeys: 2,
        plaster: 1,
        windows: [false, true, false, true],
    },
    Building {
        name: "Guild hall",
        x: [-11.0, 2.0],
        z: [-13.0, -3.0],
        storeys: 3,
        plaster: 2,
        windows: [true, true, false, true],
    },
    Building {
        name: "Bell tower",
        x: [5.0, 10.0],
        z: [-11.0, -6.0],
        storeys: 4,
        plaster: 0,
        windows: [true, true, true, true],
    },
    Building {
        name: "Chandlery",
        x: [13.0, 24.0],
        z: [-13.0, -3.0],
        storeys: 2,
        plaster: 0,
        windows: [true, false, false, true],
    },
];

/// The bell tower's belfry: posts and a roof over the tower's top.
pub const BELFRY_HEIGHT: f64 = 3.6;

/// A hanging rope: anchor (from the quarter's origin), length, and how far below the anchor
/// the hands stop (high beams: the hands stop where the roof is in reach, and the beam
/// clears the head of someone standing on the roof).
pub const ROPES: [([f64; 3], f32, f32); 3] = [
    // From the end of the townhouse's hoist beam, 0.85 m out of its street face, clear of
    // its bands (step off onto its roof at the top).
    ([6.5, 16.0, 2.15], 14.2, 1.05),
    // From the bell tower's bracket over the alley on its west side, 1.15 m out of its face.
    ([3.85, 16.0, -8.55], 14.0, 1.4),
    // From the gallows frame on the square, to swing on.
    ([-28.0, 6.6, 0.0], 5.0, 0.35),
];

/// The zip line: from the belfry roof's street edge down over the chandlery's roof and along
/// the street to its east end (jump into it off the tower's top).
pub const ZIP_FROM: [f64; 3] = [7.5, 17.4, -5.7];
pub const ZIP_TO: [f64; 3] = [27.0, 2.6, 1.5];

/// Every box of the quarter.
pub fn pieces() -> Vec<Piece> {
    let mut out = Vec::new();
    // The street, the alley and the square: cobbles on a slab (its top at 0).
    out.push(Piece::new(
        [-34.0, -0.5, -18.0],
        [28.0, 0.0, 18.0],
        Kind::Cobbles,
    ));
    for b in &BUILDINGS {
        building(&mut out, b);
    }
    belfry(&mut out);
    // Chimneys.
    for (x, z, top) in [
        (-18.0, 11.0, 12.0),
        (-8.0, 11.0, 12.0),
        (9.5, 11.5, 15.6),
        (21.0, 11.0, 8.4),
        (-20.0, -11.0, 8.4),
        (-4.0, -11.5, 12.0),
        (20.0, -11.0, 8.4),
    ] {
        out.push(Piece::new(
            [x - 0.45, top - 1.6, z - 0.45],
            [x + 0.45, top, z + 0.45],
            Kind::Stone,
        ));
    }
    // The hoist beam out over the townhouse's front, its rope hanging from the end, on a
    // post on the roof (head high over the roof, so the climber steps off under it).
    out.push(Piece::new(
        [6.3, 16.0, 1.95],
        [6.7, 16.35, 6.0],
        Kind::Timber,
    ));
    out.push(Piece::new(
        [6.3, 14.06, 5.2],
        [6.7, 16.0, 5.6],
        Kind::Timber,
    ));
    // The bell tower's bracket out over the alley, on a post on the tower's roof.
    out.push(Piece::new(
        [3.65, 16.0, -8.75],
        [7.0, 16.3, -8.35],
        Kind::Timber,
    ));
    out.push(Piece::new(
        [6.6, 14.06, -8.75],
        [7.0, 16.0, -8.35],
        Kind::Timber,
    ));
    // A plank bridge across the street between the bakery (10.5 m) and the guild hall
    // (10.5 m), with rope rails the look draws.
    out.push(Piece::new(
        [-7.2, 10.38, -3.4],
        [-5.8, 10.5, 3.4],
        Kind::Timber,
    ));
    // The gallows frame on the square: two posts and the beam.
    out.push(Piece::new(
        [-28.2, 0.0, -2.2],
        [-27.8, 7.0, -1.8],
        Kind::Timber,
    ));
    out.push(Piece::new(
        [-28.2, 0.0, 1.8],
        [-27.8, 7.0, 2.2],
        Kind::Timber,
    ));
    out.push(Piece::new(
        [-28.25, 6.6, -2.3],
        [-27.75, 7.05, 2.3],
        Kind::Timber,
    ));
    // Low starts along the street: crates stacked under a window of the chandlery, a cart by the inn, a
    // well on the square.
    for (min, max) in [
        ([14.2, 0.0, -2.6], [15.4, 1.2, -1.4]),
        ([15.6, 0.0, -2.6], [16.6, 1.0, -1.6]),
        ([14.3, 1.2, -2.5], [15.3, 2.2, -1.5]),
        ([-12.5, 0.0, 1.6], [-11.3, 1.2, 2.8]),
        // The cart's bed (the look adds wheels and shafts).
        ([-19.5, 0.6, -2.7], [-16.5, 1.3, -1.3]),
    ] {
        out.push(Piece::new(min, max, Kind::Timber));
    }
    out.push(Piece::new(
        [-31.0, 0.0, 5.0],
        [-29.0, 0.9, 7.0],
        Kind::Stone,
    ));
    out
}

/// A building's body, its bands at every floor (a cornice at the roof), and sills under its
/// windows.
fn building(out: &mut Vec<Piece>, b: &Building) {
    let h = b.height();
    let [x0, x1] = b.x;
    let [z0, z1] = b.z;
    out.push(Piece::new(
        [x0, 0.0, z0],
        [x1, h, z1],
        Kind::Plaster(b.plaster),
    ));
    out.push(Piece::new(
        [x0, h - 0.02, z0],
        [x1, h + 0.06, z1],
        Kind::Zinc,
    ));
    let d = BAND_DEPTH as f64;
    // Bands all around at every floor, the cornice (deeper) at the roof, flush with it.
    for floor in 1..=b.storeys {
        let top = floor as f64 * STOREY;
        let (dd, tall) = if floor == b.storeys {
            (d + 0.1, 0.35)
        } else {
            (d, BAND_HEIGHT as f64)
        };
        let lo = top - tall;
        out.push(Piece::new(
            [x0 - dd, lo, z0 - dd],
            [x1 + dd, top, z0],
            Kind::Stone,
        ));
        out.push(Piece::new(
            [x0 - dd, lo, z1],
            [x1 + dd, top, z1 + dd],
            Kind::Stone,
        ));
        out.push(Piece::new([x0 - dd, lo, z0], [x0, top, z1], Kind::Stone));
        out.push(Piece::new([x1, lo, z0], [x1 + dd, top, z1], Kind::Stone));
    }
    // Sills under each window, half way up each storey.
    for (face, &has) in b.windows.iter().enumerate() {
        if !has {
            continue;
        }
        for (along, _) in windows(b, face) {
            for floor in 0..b.storeys {
                let top = floor as f64 * STOREY + STOREY * 0.5;
                let lo = top - SILL_HEIGHT as f64;
                let w = WINDOW_WIDTH * 0.5 + 0.15;
                let piece = match face {
                    0 => Piece::new([x0 - d, lo, along - w], [x0, top, along + w], Kind::Stone),
                    1 => Piece::new([x1, lo, along - w], [x1 + d, top, along + w], Kind::Stone),
                    2 => Piece::new([along - w, lo, z0 - d], [along + w, top, z0], Kind::Stone),
                    _ => Piece::new([along - w, lo, z1], [along + w, top, z1 + d], Kind::Stone),
                };
                out.push(piece);
            }
        }
    }
}

/// The windows along face `face` (−X, +X, −Z, +Z) of `b`: each one's position along the
/// face (z for the X faces, x for the Z faces), evenly spaced, and the face's outward
/// normal.
pub fn windows(b: &Building, face: usize) -> Vec<(f64, Vec3)> {
    let ([a0, a1], normal) = match face {
        0 => (b.z, Vec3::NEG_X),
        1 => (b.z, Vec3::X),
        2 => (b.x, Vec3::NEG_Z),
        _ => (b.x, Vec3::Z),
    };
    let span = a1 - a0;
    let n = ((span - 1.0) / WINDOW_SPACING).floor().max(1.0) as usize;
    let step = span / n as f64;
    (0..n)
        .map(|i| (a0 + step * (i as f64 + 0.5), normal))
        .collect()
}

/// The belfry on the tower: four corner posts and a roof slab.
fn belfry(out: &mut Vec<Piece>) {
    let tower = &BUILDINGS[5];
    let h = tower.height();
    let [x0, x1] = tower.x;
    let [z0, z1] = tower.z;
    let top = h + BELFRY_HEIGHT;
    for (x, z) in [
        (x0, z0),
        (x1 - 0.5, z0),
        (x0, z1 - 0.5),
        (x1 - 0.5, z1 - 0.5),
    ] {
        out.push(Piece::new([x, h, z], [x + 0.5, top, z + 0.5], Kind::Stone));
    }
    out.push(Piece::new(
        [x0 - 0.3, top, z0 - 0.3],
        [x1 + 0.3, top + 0.4, z1 + 0.3],
        Kind::Stone,
    ));
}

/// Spawns the quarter's colliders, ropes and zip line around `demo_origin` (the movement
/// demo's origin), and its look.
pub fn spawn_city(commands: &mut Commands, demo_origin: DVec3) {
    let origin = demo_origin + DVec3::from_array(OFFSET);
    for p in pieces().iter().filter(|p| p.collides()) {
        commands.spawn((
            Name::new(NAME),
            Transform::from_translation(origin + DVec3::from_array(p.center)),
            RigidBody::fixed(),
            Collider::cuboid(Vec3::from(p.size) * 0.5),
        ));
    }
    for (i, (anchor, length, top_clear)) in ROPES.into_iter().enumerate() {
        commands.spawn((
            Name::new(format!("Climbing rope {}", i + 1)),
            Transform::from_translation(origin + DVec3::from_array(anchor)),
            ClimbRope {
                length,
                top_clear,
                ..Default::default()
            },
        ));
    }
    let from = DVec3::from_array(ZIP_FROM);
    let to = DVec3::from_array(ZIP_TO);
    commands.spawn((
        Name::new("Bell tower zip line"),
        Transform::from_translation(origin + from).with_rotation(Quat::IDENTITY),
        Zipline {
            to: (to - from).as_vec3(),
            sag: 0.9,
            end_clear: 2.0,
            exit_hop: 1.5,
        },
    ));
    crate::look::city::dress_city(commands, origin);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handholds_are_every_half_storey_up_a_windowed_face() {
        let p = pieces();
        let bakery = &BUILDINGS[0];
        // Under the first window of its street face (−Z... the bakery's street face is z0).
        let (x, _) = windows(bakery, 2)[0];
        let mut tops: Vec<f64> = p
            .iter()
            .filter(|q| q.kind == Kind::Stone)
            .filter(|q| {
                q.min().x <= x && q.max().x >= x && q.max().z <= 3.0 + 1e-6 && q.min().z >= 2.5
            })
            .map(|q| (q.max().y * 100.0).round() / 100.0)
            .collect();
        tops.sort_by(f64::total_cmp);
        tops.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
        assert_eq!(tops, vec![1.75, 3.5, 5.25, 7.0, 8.75, 10.5]);
    }

    #[test]
    fn roofs_are_a_jump_apart() {
        // The townhouse and the warehouse: 2.5 m.
        assert_eq!(BUILDINGS[2].x[0] - BUILDINGS[1].x[1], 2.5);
        // The inn and the guild hall: 2.5 m.
        assert_eq!(BUILDINGS[4].x[0] - BUILDINGS[3].x[1], 2.5);
    }
}
