//! Hairline cracks running out from holes and craters across the intact face of a brittle
//! wall (concrete, plaster, brick, stone). They are part of the wall's mesh: thin dark
//! strips just off the face, wandering and sometimes forking, wider near the damage and
//! tapering out. Everything is hashed from the grid's global cell coordinates, so a crack
//! looks the same every time its tile is rebuilt and continues across tile seams.

use engine_core::math::Vec2;

use crate::material::DestructibleMaterial;
use crate::meshing::hash3;
use crate::wall::CellGrid;

/// One straight piece of a crack, in the wall's local XY (meters), with its width at each end.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CrackSegment {
    pub a: Vec2,
    pub b: Vec2,
    pub width_a: f32,
    pub width_b: f32,
}

/// How readily a material cracks around damage (0 = never: wood splinters, steel bends).
pub fn brittleness(material: DestructibleMaterial) -> f32 {
    match material {
        DestructibleMaterial::Concrete | DestructibleMaterial::ReinforcedConcrete => 1.0,
        DestructibleMaterial::Ice => 0.9,
        DestructibleMaterial::Plaster => 0.8,
        DestructibleMaterial::Brick => 0.7,
        DestructibleMaterial::Stone => 0.6,
        DestructibleMaterial::Rock => 0.5,
        DestructibleMaterial::Wood
        | DestructibleMaterial::Metal
        | DestructibleMaterial::Glass
        | DestructibleMaterial::Dirt => 0.0,
    }
}

/// Longest crack in a fully brittle material, meters.
const MAX_LEN: f32 = 1.1;
/// Material removed from a face before it counts as damaged there, meters.
const INTACT: f32 = 0.003;

/// Cracks on the front (+Z) or back face of `grid` whose segments' midpoints lie in the cell
/// rectangle `rect` = (i0, j0, i1, j1), exclusive at the top end.
pub fn face_cracks(
    grid: &CellGrid,
    brittleness: f32,
    seed: u64,
    front: bool,
    rect: (usize, usize, usize, usize),
) -> Vec<CrackSegment> {
    let mut out = Vec::new();
    if brittleness <= 0.0 || grid.nx == 0 || grid.ny == 0 {
        return out;
    }
    let (i0, j0, i1, j1) = rect;
    let cmin = grid.cell.min_element().max(1e-3);
    let max_len = MAX_LEN * brittleness;
    let margin = (max_len / cmin).ceil() as usize + 1;
    let (ri0, rj0) = (i0.saturating_sub(margin), j0.saturating_sub(margin));
    let (ri1, rj1) = ((i1 + margin).min(grid.nx), (j1 + margin).min(grid.ny));
    let t = grid.thickness;
    let depth = |c: usize| if front { grid.front[c] } else { grid.back[c] };
    // Damage that cracks spread from: broken through, or a deep crater on this face (not a
    // built opening, which has no removed material).
    let source = |c: usize| {
        if grid.solid[c] {
            depth(c) > 0.25 * t
        } else {
            grid.front[c] + grid.back[c] > 0.0
        }
    };
    let intact = |c: usize| grid.solid[c] && depth(c) < INTACT;
    // Where a crack is drawn: this cell and its neighbors are intact, so the face is flat.
    let clean = |i: usize, j: usize| {
        for dj in -1i64..=1 {
            for di in -1i64..=1 {
                let (x, y) = (i as i64 + di, j as i64 + dj);
                if x < 0 || y < 0 || x >= grid.nx as i64 || y >= grid.ny as i64 {
                    continue;
                }
                if !intact(grid.idx(x as usize, y as usize)) {
                    return false;
                }
            }
        }
        true
    };
    let face = if front { 0 } else { 1 };
    let step = (cmin * 0.6).clamp(0.02, 0.05);
    let keep = |p: Vec2| {
        let q = (p - grid.origin) / grid.cell;
        let (i, j) = (q.x.floor() as i64, q.y.floor() as i64);
        i >= i0 as i64 && i < i1 as i64 && j >= j0 as i64 && j < j1 as i64
    };
    for j in rj0..rj1 {
        for i in ri0..ri1 {
            let c = grid.idx(i, j);
            if !source(c) {
                continue;
            }
            let (gx, gy) = (grid.global.0 + i as i64, grid.global.1 + j as i64);
            for (dir, (di, dj)) in [(1i64, 0i64), (-1, 0), (0, 1), (0, -1)]
                .into_iter()
                .enumerate()
            {
                let (ni, nj) = (i as i64 + di, j as i64 + dj);
                if ni < 0 || nj < 0 || ni >= grid.nx as i64 || nj >= grid.ny as i64 {
                    continue;
                }
                if !intact(grid.idx(ni as usize, nj as usize)) {
                    continue;
                }
                let id = gx * 8 + dir as i64 * 2 + face;
                let h = |k: i64| hash3(id, gy, k, seed);
                if h(0) > 0.2 * brittleness {
                    continue;
                }
                // Start on the shared edge, heading into the intact wall.
                let along = Vec2::new(dj as f32, di as f32) * (h(1) - 0.5) * 0.8;
                let start = grid.center(c)
                    + Vec2::new(di as f32 * 0.5, dj as f32 * 0.5) * grid.cell
                    + along * grid.cell;
                let heading =
                    Vec2::new(di as f32, dj as f32).rotate(Vec2::from_angle((h(2) - 0.5) * 1.1));
                let len = (0.25 + 0.75 * h(3)) * max_len;
                let width = 0.004 + 0.006 * h(4);
                walk(
                    grid, &clean, &keep, &h, start, heading, len, width, step, 0, &mut out,
                );
            }
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn walk(
    grid: &CellGrid,
    clean: &impl Fn(usize, usize) -> bool,
    keep: &impl Fn(Vec2) -> bool,
    h: &impl Fn(i64) -> f32,
    start: Vec2,
    heading: Vec2,
    len: f32,
    width: f32,
    step: f32,
    generation: i64,
    out: &mut Vec<CrackSegment>,
) {
    let steps = (len / step).ceil().max(1.0) as i64;
    let salt = 100 + generation * 1000;
    let mut p = start;
    let mut d = heading.normalize_or(Vec2::X);
    let mut started = false;
    let width_at = |k: i64| width * (1.0 - k as f32 / steps as f32).powf(0.7) + 0.0012;
    for k in 0..steps {
        d = d.rotate(Vec2::from_angle((h(salt + k) - 0.5) * 0.7));
        let q = p + d * step;
        let Some(c) = grid.cell_at(q) else {
            break;
        };
        let (i, j) = grid.coords(c);
        if clean(i, j) {
            started = true;
            let seg = CrackSegment {
                a: p,
                b: q,
                width_a: width_at(k),
                width_b: width_at(k + 1),
            };
            if keep((p + q) * 0.5) {
                out.push(seg);
            }
            if generation == 0 && h(salt + 500 + k) < 0.07 {
                let side = if h(salt + 700 + k) < 0.5 { -1.0 } else { 1.0 };
                let fork = d.rotate(Vec2::from_angle(side * (0.5 + 0.4 * h(salt + 900 + k))));
                let rest = (steps - k) as f32 * step * 0.55;
                walk(
                    grid,
                    clean,
                    keep,
                    h,
                    q,
                    fork,
                    rest,
                    width_at(k) * 0.6,
                    step,
                    generation + 1 + k,
                    out,
                );
            }
        } else if started {
            // Ran into damage: the crack ends there.
            break;
        }
        p = q;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material::DestructionMaterials;
    use crate::wall::{DestructibleWall, WallState};

    fn grid() -> CellGrid {
        let wall = DestructibleWall {
            material: DestructibleMaterial::Concrete,
            ..DestructibleWall::default()
        };
        WallState::new(&wall, &DestructionMaterials::default()).grid
    }

    #[test]
    fn an_intact_wall_has_no_cracks() {
        let g = grid();
        let all = (0, 0, g.nx, g.ny);
        assert!(face_cracks(&g, 1.0, 7, true, all).is_empty());
    }

    #[test]
    fn a_hole_cracks_the_wall_around_it_and_tiles_agree() {
        let mut g = grid();
        let (ci, cj) = (g.nx / 2, g.ny / 2);
        for j in cj - 2..cj + 2 {
            for i in ci - 2..ci + 2 {
                let c = g.idx(i, j);
                g.front[c] = g.thickness * 0.6;
                g.back[c] = g.thickness * 0.4;
                g.solid[c] = false;
            }
        }
        let all = face_cracks(&g, 1.0, 7, true, (0, 0, g.nx, g.ny));
        assert!(all.len() > 10, "{} segments", all.len());
        // Every segment is on intact wall.
        for s in &all {
            let c = g.cell_at((s.a + s.b) * 0.5).unwrap();
            assert!(g.solid[c] && g.front[c] < INTACT);
        }
        // Split into two halves, the pieces add up to the same cracks.
        let left = face_cracks(&g, 1.0, 7, true, (0, 0, ci, g.ny));
        let right = face_cracks(&g, 1.0, 7, true, (ci, 0, g.nx, g.ny));
        assert_eq!(left.len() + right.len(), all.len());
        // Wood doesn't crack.
        assert!(face_cracks(&g, 0.0, 7, true, (0, 0, g.nx, g.ny)).is_empty());
    }
}
