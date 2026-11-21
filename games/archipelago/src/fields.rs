//! Farm fields: every farm claims a rectangle of free, flat land round its farmyard ([`plot`],
//! shown before it is placed) as wheat fields, hop gardens or sheep paddocks, hedged or
//! fenced at the edge. A farm makes goods in proportion
//! to the fields it has; a road laid across a field takes that strip back.

use std::collections::HashSet;
use std::f32::consts::{FRAC_PI_2, TAU};
use std::sync::Arc;

use engine::prelude::*;

use crate::defs::{CELL_M, Kind};
use crate::draw::Look;
use crate::grid::{self, Grid, Occ};
use crate::island;
use crate::looks::{Piece, look};
use crate::sim::{Building, House};

/// The cells a farm works beyond its yard.
#[derive(Component, Clone, Debug, Default)]
pub struct Fields(pub Vec<IVec2>);

/// How many field cells (4 m squares) a farm wants for full output.
pub fn wanted(kind: Kind) -> usize {
    match kind {
        Kind::SheepFarm => 90,
        Kind::GrainFarm => 110,
        Kind::HopFarm => 64,
        Kind::CanePlantation => 96,
        Kind::DateGrove => 72,
        Kind::SaltWorks => 48,
        _ => 0,
    }
}

pub fn is_farm(kind: Kind) -> bool {
    wanted(kind) > 0
}

/// Flat, dry ground a field can lie on.
fn cell_fieldable(c: IVec2) -> bool {
    let (lo, hi) = grid::cell_range(c);
    lo > 1.2 && hi < 70.0 && hi - lo < 1.4
}

/// The rectangle of fields a farm keeps round its yard, in the yard's own cells (x along its
/// right, z toward its front; the yard is `0..w` by `0..d`): as wide again as the yard, half
/// of it to each side, and deep enough behind it for [`wanted`] cells. Nothing lies in front
/// of the yard, where its road comes in.
fn plot_size(kind: Kind) -> Option<(i32, i32)> {
    let want = wanted(kind) as i32;
    if want == 0 {
        return None;
    }
    let (w, d) = kind.footprint(0);
    let side = (w + 1) / 2;
    let width = w + 2 * side;
    let back = ((want + w * d) + width - 1) / width - d;
    Some((side, back.max(1)))
}

/// The cells of a farm's field plot (not its yard) for the farm turned `rot` at `origin`.
pub fn plot(kind: Kind, origin: IVec2, rot: u8) -> Vec<IVec2> {
    let Some((side, back)) = plot_size(kind) else {
        return Vec::new();
    };
    let (w, d) = kind.footprint(0);
    let mid = grid::footprint_center(kind, origin, rot);
    let turn = |x: f64, z: f64| match rot % 4 {
        0 => (x, z),
        1 => (z, -x),
        2 => (-x, -z),
        _ => (-z, x),
    };
    let mut out = Vec::new();
    for z in -back..d {
        for x in -side..w + side {
            if (0..w).contains(&x) && (0..d).contains(&z) {
                continue;
            }
            // The cell's middle from the yard's middle, in the yard's space, then turned.
            let (lx, lz) = (
                x as f64 + 0.5 - w as f64 * 0.5,
                z as f64 + 0.5 - d as f64 * 0.5,
            );
            let (dx, dz) = turn(lx, lz);
            out.push(grid::cell_of(mid.x + dx * CELL_M, mid.y + dz * CELL_M));
        }
    }
    out
}

/// Whether a plot cell can be sown now (free, flat, on the farm's island).
fn can_sow(g: &Grid, isle: usize, c: IVec2) -> bool {
    g.is_free(c) && cell_fieldable(c) && {
        let p = grid::cell_center(c);
        island::island_at(p.x, p.y) == Some(isle)
    }
}

/// A farm about to be placed: each cell of its plot and whether a field will lie there.
pub fn preview(g: &Grid, kind: Kind, origin: IVec2, rot: u8, isle: usize) -> Vec<(IVec2, bool)> {
    plot(kind, origin, rot)
        .into_iter()
        .map(|c| (c, can_sow(g, isle, c)))
        .collect()
}

/// Cells `b` takes as fields now: the free ones of its plot.
fn candidates(g: &Grid, b: &Building) -> Vec<IVec2> {
    plot(b.kind, b.origin, b.rot)
        .into_iter()
        .filter(|c| can_sow(g, b.island, *c))
        .collect()
}

/// Give a farm just placed its fields.
pub fn claim(world: &mut World, farm: Entity) {
    let Some(b) = world.get::<Building>(farm).cloned() else {
        return;
    };
    if !is_farm(b.kind) {
        return;
    }
    let cells = candidates(world.resource::<Grid>(), &b);
    {
        let mut g = world.resource_mut::<Grid>();
        for &c in &cells {
            g.cells.insert(c, Occ::Field(farm));
        }
    }
    crate::build::clear_trees(world, &cells);
    match b.kind {
        Kind::SheepFarm => {}
        Kind::SaltWorks => crate::build::paint_cells(world, &cells, "sand"),
        _ => crate::build::paint_cells(world, &cells, "dirt"),
    }
    world.entity_mut(farm).insert(Fields(cells));
    redraw(world, farm);
}

/// Take one cell back from the farm that works it (a road is going across).
pub fn release_cell(world: &mut World, c: IVec2) {
    let Some(Occ::Field(farm)) = world.resource::<Grid>().cells.get(&c).copied() else {
        return;
    };
    world.resource_mut::<Grid>().cells.remove(&c);
    if let Some(mut f) = world.get_mut::<Fields>(farm) {
        f.0.retain(|x| *x != c);
    }
    redraw(world, farm);
}

/// Give back all of a farm's fields (it is being demolished).
pub fn release_all(world: &mut World, farm: Entity) {
    let Some(f) = world.get::<Fields>(farm).cloned() else {
        return;
    };
    {
        let mut g = world.resource_mut::<Grid>();
        for c in &f.0 {
            if g.cells.get(c) == Some(&Occ::Field(farm)) {
                g.cells.remove(c);
            }
        }
    }
    crate::build::paint_cells(world, &f.0, "grass");
}

/// Share of a farm's wanted fields it has, 0..1.
pub fn share(kind: Kind, fields: Option<&Fields>) -> f32 {
    share_of(kind, fields.map_or(0, |f| f.0.len()))
}

/// Share of full output a farm makes with `have` field cells, 0..1.
pub fn share_of(kind: Kind, have: usize) -> f32 {
    let want = wanted(kind);
    if want == 0 {
        return 1.0;
    }
    (have as f32 / (want as f32 * 0.8)).min(1.0)
}

fn h01(seed: u64, k: u64) -> f32 {
    let mut x = seed ^ k.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
    x ^= x >> 33;
    (x >> 40) as f32 / (1u64 << 24) as f32
}

/// The farm's look: its yard plus a piece set for every field cell and the hedges round them.
pub fn redraw(world: &mut World, farm: Entity) {
    let Some(b) = world.get::<Building>(farm).cloned() else {
        return;
    };
    let tier = world
        .get::<House>(farm)
        .map_or(crate::defs::Tier::Fisherfolk, |h| h.tier);
    let cells = world
        .get::<Fields>(farm)
        .map(|f| f.0.clone())
        .unwrap_or_default();
    let biome = island::islands()[b.island].biome();
    let mut pieces = look(b.kind, tier, b.seed, biome);
    pieces.extend(field_pieces(&b, &cells));
    world.entity_mut(farm).insert(Look(Arc::new(pieces)));
    world.resource_mut::<crate::draw::Drawn>().dirty = true;
}

/// Pieces for a farm's fields, in the farm's own space.
pub fn field_pieces(b: &Building, cells: &[IVec2]) -> Vec<Piece> {
    let mid = grid::footprint_center(b.kind, b.origin, b.rot);
    let yaw = grid::yaw(b.rot);
    let to_local = Quat::from_rotation_y(-yaw);
    let local = |c: IVec2, off: Vec2| {
        let p = grid::cell_center(c);
        let d = DVec3::new(
            p.x + off.x as f64 - mid.x,
            grid::cell_height(c) - b.base_y,
            p.y + off.y as f64 - mid.y,
        );
        to_local * d.as_vec3()
    };
    let yard: HashSet<IVec2> = grid::footprint(b.kind, b.origin, b.rot)
        .into_iter()
        .collect();
    let field: HashSet<IVec2> = cells.iter().copied().collect();
    let front = grid::front_dir(b.rot);
    let mut out = Vec::new();
    let s = b.seed;
    for (k, &c) in cells.iter().enumerate() {
        let k = k as u64;
        // Fields come in strips a few cells wide, each sown or grazed a little differently.
        let lc = to_local * Vec3::new(c.x as f32, 0.0, c.y as f32);
        let strip = (lc.x / 4.0).floor() as i64 as u64;
        let strip_t = h01(s, 9000u64.wrapping_add(strip));
        match b.kind {
            Kind::GrainFarm => {
                let ripe = 0.82 + 0.26 * strip_t;
                let green = if strip_t < 0.18 { 0.8 } else { 1.0 };
                out.push(Piece::at("wheat_cell", local(c, Vec2::ZERO)).tint([
                    ripe * green,
                    ripe * (0.97 + 0.05 * green),
                    0.85 * green,
                ]));
            }
            Kind::CanePlantation => {
                out.push(crate::regional::cane_piece(
                    s,
                    7000 + k,
                    local(c, Vec2::ZERO),
                    strip_t,
                ));
            }
            Kind::DateGrove => {
                out.push(crate::regional::palm_piece(
                    s,
                    7000 + k,
                    local(c, Vec2::ZERO),
                ));
            }
            Kind::SaltWorks => {
                // Pans in pairs of strips: filling, rose, crusting, dry.
                let pair = h01(s, 9100u64.wrapping_add(strip / 2));
                let at = local(c, Vec2::ZERO);
                out.extend(
                    crate::regional::pan_pieces(s, 7000 + k, Vec3::ZERO, pair)
                        .into_iter()
                        .map(|mut p| {
                            p.pos += at;
                            p.yaw -= yaw;
                            p
                        }),
                );
            }
            Kind::HopFarm => {
                for (i, (dx, dz)) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)]
                    .into_iter()
                    .enumerate()
                {
                    let off = Quat::from_rotation_y(yaw) * Vec3::new(dx, 0.0, dz);
                    out.push(
                        Piece::at("hop_pole", local(c, Vec2::new(off.x, off.z)))
                            .yaw(h01(s, 2000 + k * 4 + i as u64) * TAU)
                            .scale(0.85 + 0.25 * strip_t),
                    );
                }
            }
            _ => {
                // A sheep on about one cell in three.
                if h01(s, 3000 + k) < 0.36 {
                    let off = Vec2::new(h01(s, 3100 + k) * 3.0 - 1.5, h01(s, 3200 + k) * 3.0 - 1.5);
                    let t = 0.85 + 0.25 * h01(s, 3300 + k);
                    out.push(
                        Piece::at("sheep", local(c, off))
                            .yaw(h01(s, 3400 + k) * TAU)
                            .scale(0.9 + 0.2 * h01(s, 3500 + k))
                            .tint([t, t, t * 0.97]),
                    );
                }
            }
        }
    }
    // Hedges (fences round paddocks) on every outer edge of fields and yard, but not across
    // the yard's front where the road comes in.
    let edge_model = match b.kind {
        Kind::SheepFarm => "fence_4",
        Kind::DateGrove => "mud_wall_4",
        // Cane grows to the edge of its field; a pan's bund is its own edge.
        Kind::CanePlantation | Kind::SaltWorks => return out,
        _ => "hedge_4",
    };
    let half = CELL_M as f32 * 0.5;
    let mut k = 0u64;
    for &c in yard.iter().chain(cells.iter()) {
        for d in [IVec2::X, IVec2::NEG_X, IVec2::Y, IVec2::NEG_Y] {
            let n = c + d;
            if field.contains(&n) || yard.contains(&n) {
                continue;
            }
            if yard.contains(&c) && d == front {
                continue;
            }
            k += 1;
            let wyaw = if d.x != 0 { FRAC_PI_2 } else { 0.0 };
            let off = Vec2::new(d.x as f32, d.y as f32) * (half - 0.2);
            out.push(
                Piece::at(edge_model, local(c, off))
                    .yaw(wyaw - yaw)
                    .scale(1.0 + 0.06 * h01(s, 5000 + k))
                    .tint([0.9 + 0.15 * h01(s, 5100 + k); 3]),
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_farms_plot_is_a_rectangle_beside_and_behind_its_yard() {
        let o = IVec2::new(40, -20);
        for kind in [
            Kind::SheepFarm,
            Kind::GrainFarm,
            Kind::HopFarm,
            Kind::DateGrove,
            Kind::SaltWorks,
            Kind::CanePlantation,
        ] {
            for rot in 0..4 {
                let plot = plot(kind, o, rot);
                assert!(plot.len() >= wanted(kind), "{kind:?} {rot}: {}", plot.len());
                let yard: HashSet<IVec2> = grid::footprint(kind, o, rot).into_iter().collect();
                let cells: HashSet<IVec2> = plot.iter().copied().collect();
                assert_eq!(cells.len(), plot.len(), "{kind:?} {rot}: a cell twice");
                assert!(
                    cells.is_disjoint(&yard),
                    "{kind:?} {rot}: a field on the yard"
                );
                // Yard and fields together fill their bounding box.
                let all: Vec<IVec2> = yard.iter().chain(cells.iter()).copied().collect();
                let lo = all.iter().fold(IVec2::MAX, |a, b| a.min(*b));
                let hi = all.iter().fold(IVec2::MIN, |a, b| a.max(*b));
                let area = (hi.x - lo.x + 1) * (hi.y - lo.y + 1);
                assert_eq!(area as usize, all.len(), "{kind:?} {rot}: not a rectangle");
                // Nothing in front of the yard, and as much to its left as to its right.
                let f = grid::front_dir(rot);
                let edge = yard.iter().map(|c| c.dot(f)).max().unwrap();
                assert!(cells.iter().all(|c| c.dot(f) <= edge), "{kind:?} {rot}");
                assert_eq!(
                    grid::footprint_center(kind, o, rot).dot(f.perp().as_dvec2()),
                    (lo.as_dvec2() + hi.as_dvec2() + DVec2::ONE).dot(f.perp().as_dvec2())
                        * 0.5
                        * CELL_M,
                    "{kind:?} {rot}: off centre"
                );
            }
        }
    }

    #[test]
    fn farms_take_fields_behind_their_yard_and_give_them_back() {
        let mut world = crate::town::tests::bare_world();
        let hub = island::home_town_spot() + DVec2::new(-260.0, 120.0);
        let farm = crate::town::place_near(&mut world, Kind::SheepFarm, hub, 40, false)
            .expect("a sheep farm site on Haverholm");
        let b = world.get::<Building>(farm).cloned().unwrap();
        let fields = world.get::<Fields>(farm).cloned().unwrap();
        assert!(
            fields.0.len() > wanted(Kind::SheepFarm) / 2,
            "{} fields",
            fields.0.len()
        );
        let front = grid::front_dir(b.rot);
        let edge = grid::footprint(b.kind, b.origin, b.rot)
            .iter()
            .map(|c| c.dot(front))
            .max()
            .unwrap();
        assert!(
            fields.0.iter().all(|c| c.dot(front) <= edge),
            "fields in front of the yard"
        );
        // A road across a field takes that cell back.
        let c = fields.0[fields.0.len() / 2];
        crate::build::place_road(&mut world, c, false).unwrap();
        assert!(!world.get::<Fields>(farm).unwrap().0.contains(&c));
        crate::build::demolish(&mut world, farm);
        let g = world.resource::<Grid>();
        assert!(g.cells.values().all(|o| *o != Occ::Field(farm)));
    }
}
