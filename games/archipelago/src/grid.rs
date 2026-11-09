//! The build grid: 4 m cells over the whole archipelago, what stands on each, and whether a
//! building fits somewhere.
//!
//! A building's footprint at rotation 0 spans `w` cells along +X and `d` along +Z from its
//! origin cell, with its front (door, quay) facing +Z. Each quarter turn turns it 90° about Y
//! (`yaw = rot · 90°`, so the front faces +X, −Z, −X in turn) and swaps `w` and `d`.

use std::collections::{HashMap, HashSet};

use engine::prelude::*;

use crate::defs::{CELL_M, Kind, Site};
use crate::island;

/// What stands on a cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Occ {
    Building(Entity),
    Road,
    /// A farm's field: blocks buildings, but a road may be laid across it.
    Field(Entity),
}

/// Every occupied cell.
#[derive(Resource, Default, Debug)]
pub struct Grid {
    pub cells: HashMap<IVec2, Occ>,
    pub roads: HashSet<IVec2>,
}

impl Grid {
    pub fn is_free(&self, c: IVec2) -> bool {
        !self.cells.contains_key(&c)
    }

    /// Free, or only a field (a road may go there).
    pub fn is_open(&self, c: IVec2) -> bool {
        matches!(self.cells.get(&c), None | Some(Occ::Field(_)))
    }

    pub fn is_road(&self, c: IVec2) -> bool {
        self.roads.contains(&c)
    }
}

pub fn cell_of(x: f64, z: f64) -> IVec2 {
    IVec2::new((x / CELL_M).floor() as i32, (z / CELL_M).floor() as i32)
}

/// World X, Z of a cell's middle.
pub fn cell_center(c: IVec2) -> DVec2 {
    DVec2::new((c.x as f64 + 0.5) * CELL_M, (c.y as f64 + 0.5) * CELL_M)
}

/// Ground height at a cell's middle.
pub fn cell_height(c: IVec2) -> f64 {
    let p = cell_center(c);
    island::height(p.x, p.y)
}

/// Cells' ground ranges, measured once (the islands' shape never changes).
static RANGES: std::sync::LazyLock<std::sync::RwLock<HashMap<IVec2, (f64, f64)>>> =
    std::sync::LazyLock::new(Default::default);

/// Lowest and highest ground over a cell (corners and middle).
pub fn cell_range(c: IVec2) -> (f64, f64) {
    if let Some(r) = RANGES.read().ok().and_then(|m| m.get(&c).copied()) {
        return r;
    }
    let r = measure_range(c);
    if let Ok(mut m) = RANGES.write() {
        m.insert(c, r);
    }
    r
}

fn measure_range(c: IVec2) -> (f64, f64) {
    let (x0, z0) = (c.x as f64 * CELL_M, c.y as f64 * CELL_M);
    let mut lo = f64::MAX;
    let mut hi = f64::MIN;
    for (dx, dz) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0), (0.5, 0.5)] {
        let h = island::height(x0 + dx * CELL_M, z0 + dz * CELL_M);
        lo = lo.min(h);
        hi = hi.max(h);
    }
    (lo, hi)
}

/// Dry, fairly flat land a building or road may stand on.
pub fn cell_buildable(c: IVec2) -> bool {
    let (lo, hi) = cell_range(c);
    lo > 0.9 && hi < 70.0 && hi - lo < 1.3
}

/// Dry ground a road may cross: beaches and gentle slopes too.
pub fn cell_roadable(c: IVec2) -> bool {
    let (lo, hi) = cell_range(c);
    lo > 0.3 && hi < 70.0 && hi - lo < 1.6
}

/// Water at least `depth` meters deep at the cell's middle.
pub fn cell_sea(c: IVec2, depth: f64) -> bool {
    cell_height(c) < -depth
}

/// Unit step toward the front for a rotation (cells).
pub fn front_dir(rot: u8) -> IVec2 {
    match rot % 4 {
        0 => IVec2::new(0, 1),
        1 => IVec2::new(1, 0),
        2 => IVec2::new(0, -1),
        _ => IVec2::new(-1, 0),
    }
}

/// Yaw of a rotation, radians (local +Z turns toward [`front_dir`]).
pub fn yaw(rot: u8) -> f32 {
    (rot % 4) as f32 * std::f32::consts::FRAC_PI_2
}

/// Cells a footprint covers.
pub fn footprint(kind: Kind, origin: IVec2, rot: u8) -> Vec<IVec2> {
    let (w, d) = kind.footprint(rot);
    let mut v = Vec::with_capacity((w * d) as usize);
    for z in 0..d {
        for x in 0..w {
            v.push(origin + IVec2::new(x, z));
        }
    }
    v
}

/// The row of cells just outside the footprint's front.
pub fn front_row(kind: Kind, origin: IVec2, rot: u8) -> Vec<IVec2> {
    let (w, d) = kind.footprint(rot);
    match rot % 4 {
        0 => (0..w).map(|x| origin + IVec2::new(x, d)).collect(),
        1 => (0..d).map(|z| origin + IVec2::new(w, z)).collect(),
        2 => (0..w).map(|x| origin + IVec2::new(x, -1)).collect(),
        _ => (0..d).map(|z| origin + IVec2::new(-1, z)).collect(),
    }
}

/// Cells around the footprint (one ring), where a road must touch.
pub fn ring(kind: Kind, origin: IVec2, rot: u8) -> Vec<IVec2> {
    let (w, d) = kind.footprint(rot);
    let mut v = Vec::new();
    for x in -1..=w {
        v.push(origin + IVec2::new(x, -1));
        v.push(origin + IVec2::new(x, d));
    }
    for z in 0..d {
        v.push(origin + IVec2::new(-1, z));
        v.push(origin + IVec2::new(w, z));
    }
    v
}

/// World X, Z of a footprint's middle.
pub fn footprint_center(kind: Kind, origin: IVec2, rot: u8) -> DVec2 {
    let (w, d) = kind.footprint(rot);
    DVec2::new(
        (origin.x as f64 + w as f64 * 0.5) * CELL_M,
        (origin.y as f64 + d as f64 * 0.5) * CELL_M,
    )
}

/// Why a building can't go somewhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blocked {
    Occupied,
    NotLand,
    Steep,
    NeedsCoast,
    WrongIsland,
    Infertile,
}

impl Blocked {
    pub fn message(self) -> &'static str {
        match self {
            Blocked::Occupied => "Something already stands there.",
            Blocked::NotLand => "That isn't dry land.",
            Blocked::Steep => "The ground is too steep there.",
            Blocked::NeedsCoast => "It must face open water a few meters deep.",
            Blocked::WrongIsland => "The footprint crosses from one island to another.",
            Blocked::Infertile => "This island's ground can't grow or give that.",
        }
    }
}

/// Whether `kind` fits at `origin` turned `rot`, and which island it would stand on.
pub fn check(grid: &Grid, kind: Kind, origin: IVec2, rot: u8) -> Result<usize, Blocked> {
    let def = kind.def();
    let cells = footprint(kind, origin, rot);
    let mut isle = None;
    for &c in &cells {
        if !grid.is_free(c) {
            return Err(Blocked::Occupied);
        }
        let p = cell_center(c);
        let here = island::island_at(p.x, p.y);
        match def.site {
            Site::Land => {
                let Some(i) = here else {
                    return Err(Blocked::NotLand);
                };
                if isle.is_some_and(|j| j != i) {
                    return Err(Blocked::WrongIsland);
                }
                isle = Some(i);
                if !cell_buildable(c) {
                    let (lo, _) = cell_range(c);
                    return Err(if lo <= 0.9 {
                        Blocked::NotLand
                    } else {
                        Blocked::Steep
                    });
                }
            }
            Site::Coast => {
                // Quays stand on the beach or out over the shallows on piles.
                let (lo, hi) = cell_range(c);
                if hi > 9.0 || hi - lo > 3.0 {
                    return Err(Blocked::Steep);
                }
                if lo < -6.0 {
                    return Err(Blocked::NeedsCoast);
                }
            }
        }
    }
    if def.site == Site::Coast {
        // Open water a few meters deep within three cells of every front cell.
        let front = front_row(kind, origin, rot);
        let out = front_dir(rot);
        if !front
            .iter()
            .all(|&c| (0..3).any(|k| cell_sea(c + out * k, 2.5)))
        {
            return Err(Blocked::NeedsCoast);
        }
        // Part of it must stand on land, so a road can reach its back.
        let c = footprint_center(kind, origin, rot);
        isle = Some(island::nearest_island(c.x, c.y));
        if !cells.iter().any(|&c| cell_height(c) > 0.6) {
            return Err(Blocked::NeedsCoast);
        }
    }
    let i = isle.ok_or(Blocked::NotLand)?;
    if def.needs_fertility && !island::islands()[i].fertility.allows(kind) {
        return Err(Blocked::Infertile);
    }
    Ok(i)
}

/// Height a building's floor stands at: the highest ground under it (quays a little above
/// the sea).
pub fn base_height(kind: Kind, origin: IVec2, rot: u8) -> f64 {
    let mut hi = f64::MIN;
    for c in footprint(kind, origin, rot) {
        hi = hi.max(cell_range(c).1);
    }
    if kind.def().site == Site::Coast {
        hi.max(1.6)
    } else {
        hi
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn footprints_turn_and_fronts_face_out() {
        let o = IVec2::new(10, 10);
        assert_eq!(footprint(Kind::Sawmill, o, 0).len(), 12);
        assert_eq!(Kind::Sawmill.footprint(1), (4, 3));
        let f = front_row(Kind::Sawmill, o, 0);
        assert!(f.iter().all(|c| c.y == 14));
        let f = front_row(Kind::Sawmill, o, 1);
        assert!(f.iter().all(|c| c.x == 14));
        assert_eq!(ring(Kind::House, o, 0).len(), 16);
    }
}
