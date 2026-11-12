//! What the plants hide: grass, herbs, ferns, dwarf shrubs and bramble, and the trunks of a
//! wood, published to `engine_world::Obscurance` under the name "foliage", so NPC AI can find
//! cover (a hare bolting into tall grass, a deer into the trees, a wolf lying in wait) and lose
//! sight of a player crouched in it, without depending on this crate.
//!
//! Each standing [`GrassField`] (meadow grass and the forest floor's layers; what lies flat,
//! leaf litter and twigs, hides nothing) becomes a grid of columns, one per map point: from the
//! ground up to the blades' top, an extinction from how many blades stand there and how wide
//! they are. A canopy of thin elements (blades, leaves) dims sight by
//! `G × (elements per m²) × (element width)`, G ≈ 0.5 for elements turned every which way,
//! about 0.6 of a tapering blade's width counting (the leaf-area density form of Beer's law,
//! Ross 1981; Monsi and Saeki 1953). Meadow grass at full density then lets about a fifth of
//! the light through a meter of it, bramble almost none.
//!
//! Trees add a coarse grid of their trunks: a wood with `n` trunks of diameter `d` per square
//! meter has a mean free sight line of `1 / (n d)` (the forest visibility of Petrov's
//! line-intercept formula, used in forest acoustics and line-of-sight models), so a column of
//! the grid dims sight by the sum of its trunks' diameters over its area, from the ground to
//! the crown's foot. A single trunk is a statistical haze here, not a solid: NPC sight that
//! also casts rays at tree colliders counts nearby trunks twice.
//!
//! The snapshot is rebuilt when fields or trees change (streamed in, grown, felled, shifted),
//! at most twice a second.

use std::sync::Arc;

use engine_core::math::{DAffine3, DVec2, DVec3};
use engine_core::prelude::*;
use engine_render::{GrassField, GrassStyle};
use engine_world::{Obscurance, ObscuranceQuery};

use crate::plant::Plant;
use crate::scenery_tree::SceneryTree;
use crate::species::PlantLibrary;

/// The name foliage registers under in [`Obscurance`].
pub const SOURCE: &str = "foliage";

/// The densest a column gets, 1/m (bramble: sight ends within about 40 cm).
const MAX_EXTINCTION: f32 = 8.0;

/// Trunk grid cells, meters on a side.
const WOOD_CELL_M: f64 = 4.0;

/// The most cells the trunk grid holds (beyond, its cells grow).
const MAX_WOOD_CELLS: usize = 1 << 22;

/// Seconds between rebuilds at most.
const REBUILD_SECS: f32 = 0.5;

/// Extinction per meter of a stand of `style` at `density` (0..1), and how tall it stands.
/// `None` for what lies on the ground.
pub fn stand_of(style: &GrassStyle, density: f32) -> Option<(f32, f32)> {
    if style.lying > 0.5 || density <= 0.0 {
        return None;
    }
    let d = density.min(1.0);
    // The cull shader grows blades `mix(0.55, 1, density)` tall and keeps `density` of them;
    // a bent blade's tip stands lower.
    let top = style.height * (0.55 + 0.45 * d) * (1.0 - 0.25 * style.bend.min(1.0));
    let extinction = 0.5 * 0.6 * style.blades_per_m2 * d * style.width;
    (top > 0.01 && extinction > 0.0).then_some((extinction.min(MAX_EXTINCTION), top))
}

/// A grid of columns in a frame of its own: column `(i, j)` covers local x `x0 + i·cx ..`,
/// z `z0 + j·cz ..`, and from local height `lo` to `hi`, with extinction `ext` per world meter.
#[derive(Debug, Clone)]
struct Columns {
    to_local: DAffine3,
    x0: f64,
    z0: f64,
    cell: DVec2,
    nx: usize,
    nz: usize,
    lo: Vec<f32>,
    hi: Vec<f32>,
    ext: Vec<f32>,
}

impl Columns {
    fn index(&self, x: f64, z: f64) -> Option<usize> {
        let i = ((x - self.x0) / self.cell.x).floor();
        let j = ((z - self.z0) / self.cell.y).floor();
        (i >= 0.0 && j >= 0.0 && (i as usize) < self.nx && (j as usize) < self.nz)
            .then(|| j as usize * self.nx + i as usize)
    }

    fn extinction(&self, p: DVec3) -> f32 {
        let q = self.to_local.transform_point3(p);
        match self.index(q.x, q.z) {
            Some(k) if q.y >= self.lo[k] as f64 && q.y <= self.hi[k] as f64 => self.ext[k],
            _ => 0.0,
        }
    }

    /// Optical depth along `from`..`to` (world), `length` meters long: the columns crossed,
    /// each over the stretch of the segment inside its height range.
    fn optical_depth(&self, from: DVec3, to: DVec3, length: f64) -> f64 {
        let a = self.to_local.transform_point3(from);
        let b = self.to_local.transform_point3(to);
        let d = b - a;
        // Clip to the grid's rectangle (t along the segment, 0..1).
        let (mut t0, mut t1) = (0.0f64, 1.0f64);
        let x1 = self.x0 + self.cell.x * self.nx as f64;
        let z1 = self.z0 + self.cell.y * self.nz as f64;
        for (p, dp, lo, hi) in [(a.x, d.x, self.x0, x1), (a.z, d.z, self.z0, z1)] {
            if dp.abs() < 1e-12 {
                if p < lo || p >= hi {
                    return 0.0;
                }
            } else {
                let (u, v) = ((lo - p) / dp, (hi - p) / dp);
                t0 = t0.max(u.min(v));
                t1 = t1.min(u.max(v));
            }
        }
        if t0 >= t1 {
            return 0.0;
        }
        // Walk the cells the segment's ground track crosses (Amanatides and Woo).
        let start = a + d * t0;
        let cell_of =
            |v: f64, o: f64, c: f64, n: usize| (((v - o) / c).floor().max(0.0) as usize).min(n - 1);
        let mut i = cell_of(start.x, self.x0, self.cell.x, self.nx);
        let mut j = cell_of(start.z, self.z0, self.cell.y, self.nz);
        let axis = |dp: f64, p0: f64, o: f64, c: f64, k: usize| -> (i64, f64, f64) {
            if dp > 0.0 {
                (1, (o + (k + 1) as f64 * c - p0) / dp, c / dp)
            } else if dp < 0.0 {
                (-1, (o + k as f64 * c - p0) / dp, -c / dp)
            } else {
                (0, f64::INFINITY, f64::INFINITY)
            }
        };
        let (si, mut ti, di) = axis(d.x, a.x, self.x0, self.cell.x, i);
        let (sj, mut tj, dj) = axis(d.z, a.z, self.z0, self.cell.y, j);
        let mut t = t0;
        let mut depth = 0.0;
        loop {
            let next = ti.min(tj).min(t1);
            let k = j * self.nx + i;
            let e = self.ext[k];
            if e > 0.0 && next > t {
                depth += e as f64 * self.inside_height(a.y, d.y, t, next, k) * length;
            }
            if next >= t1 {
                break;
            }
            t = next;
            if ti <= tj {
                let n = i as i64 + si;
                if n < 0 || n >= self.nx as i64 {
                    break;
                }
                i = n as usize;
                ti += di;
            } else {
                let n = j as i64 + sj;
                if n < 0 || n >= self.nz as i64 {
                    break;
                }
                j = n as usize;
                tj += dj;
            }
        }
        depth
    }

    /// The part of `t0..t1` where height `y0 + t·dy` is within column `k`'s range.
    fn inside_height(&self, y0: f64, dy: f64, t0: f64, t1: f64, k: usize) -> f64 {
        let (lo, hi) = (self.lo[k] as f64, self.hi[k] as f64);
        let (u, v) = if dy.abs() < 1e-12 {
            if y0 < lo || y0 > hi {
                return 0.0;
            }
            (t0, t1)
        } else {
            let (u, v) = ((lo - y0) / dy, (hi - y0) / dy);
            (u.min(v).max(t0), u.max(v).min(t1))
        };
        (v - u).max(0.0)
    }
}

/// Plants as sight sees them, frozen when they last changed.
#[derive(Debug, Clone, Default)]
pub struct FoliageCover {
    grids: Vec<Arc<Columns>>,
}

impl FoliageCover {
    /// Columns for a field (its world transform `world`): one per map point, centered on it.
    fn add_field(&mut self, field: &GrassField, world: DAffine3) {
        let [rx, rz] = field.resolution;
        let n = (rx * rz) as usize;
        if rx < 2 || rz < 2 || field.density.len() < n || field.heights.len() < n {
            return;
        }
        let palette: Vec<&GrassStyle> = field.palette().collect();
        let mut lo = Vec::with_capacity(n);
        let mut hi = Vec::with_capacity(n);
        let mut ext = Vec::with_capacity(n);
        let mut any = false;
        for k in 0..n {
            let kind = field.kinds.get(k).copied().unwrap_or(0) as usize;
            let style = palette.get(kind).copied().unwrap_or(&field.style);
            let ground = field.heights[k];
            let (e, top) = stand_of(style, field.density[k]).unwrap_or((0.0, 0.0));
            any |= e > 0.0;
            lo.push(ground);
            hi.push(ground + top);
            ext.push(e);
        }
        if !any {
            return;
        }
        let cell = DVec2::new(
            field.size[0] as f64 / (rx - 1) as f64,
            field.size[1] as f64 / (rz - 1) as f64,
        );
        self.grids.push(Arc::new(Columns {
            to_local: world.inverse(),
            x0: -field.size[0] as f64 * 0.5 - cell.x * 0.5,
            z0: -field.size[1] as f64 * 0.5 - cell.y * 0.5,
            cell,
            nx: rx as usize,
            nz: rz as usize,
            lo,
            hi,
            ext,
        }));
    }

    /// The trunk grid from trees standing at `foot` with trunk `radius` and crown foot
    /// `height` above it, meters.
    fn add_trunks(&mut self, trunks: &[(DVec3, f32, f32)]) {
        let Some(first) = trunks.first() else { return };
        let (mut min, mut max) = (first.0, first.0);
        for (p, ..) in trunks {
            min = min.min(*p);
            max = max.max(*p);
        }
        let mut cell = WOOD_CELL_M;
        let dims = |cell: f64| {
            (
                ((max.x - min.x) / cell).floor() as usize + 1,
                ((max.z - min.z) / cell).floor() as usize + 1,
            )
        };
        while dims(cell).0 * dims(cell).1 > MAX_WOOD_CELLS {
            cell *= 2.0;
        }
        let (nx, nz) = dims(cell);
        let n = nx * nz;
        let mut lo = vec![f32::INFINITY; n];
        let mut hi = vec![f32::NEG_INFINITY; n];
        let mut ext = vec![0.0f32; n];
        let area = (cell * cell) as f32;
        for (p, radius, height) in trunks {
            let i = (((p.x - min.x) / cell) as usize).min(nx - 1);
            let j = (((p.z - min.z) / cell) as usize).min(nz - 1);
            let k = j * nx + i;
            lo[k] = lo[k].min(p.y as f32);
            hi[k] = hi[k].max(p.y as f32 + height);
            ext[k] += 2.0 * radius / area;
        }
        for k in 0..n {
            if ext[k] <= 0.0 {
                (lo[k], hi[k]) = (0.0, 0.0);
            }
            ext[k] = ext[k].min(MAX_EXTINCTION);
        }
        self.grids.push(Arc::new(Columns {
            to_local: DAffine3::IDENTITY,
            x0: min.x,
            z0: min.z,
            cell: DVec2::splat(cell),
            nx,
            nz,
            lo,
            hi,
            ext,
        }));
    }
}

impl ObscuranceQuery for FoliageCover {
    fn transmittance(&self, from: DVec3, to: DVec3) -> f32 {
        let length = from.distance(to);
        if length <= 0.0 {
            return 1.0;
        }
        let depth: f64 = self
            .grids
            .iter()
            .map(|g| g.optical_depth(from, to, length))
            .sum();
        (-depth).exp() as f32
    }

    fn extinction(&self, p: DVec3) -> f32 {
        self.grids.iter().map(|g| g.extinction(p)).sum()
    }
}

/// A tree's trunk for the sight grid: its radius and the height of its crown's foot, meters,
/// from its size (0..1 of mature) and genes.
fn trunk(
    library: &PlantLibrary,
    species: crate::species::SpeciesId,
    size: f32,
    height: f32,
) -> Option<(f32, f32)> {
    let sp = library.get(species);
    if !sp.is_tree() || size <= 0.05 {
        return None;
    }
    Some((
        sp.trunk_radius_m * size,
        // The bole below the crown: about half the tree's height.
        0.5 * sp.max_height_m * size * height,
    ))
}

/// The last built parts: the fields' columns and the trunk grid, each rebuilt only when what
/// it is made from changes.
#[derive(Default)]
pub struct Built {
    fields: Vec<Arc<Columns>>,
    trunks: Vec<Arc<Columns>>,
    fields_dirty: bool,
    trunks_dirty: bool,
    wait: f32,
}

/// Rebuild the snapshot when fields or trees change, and publish it.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn publish_cover(
    time: Res<Time>,
    mut built: Local<Built>,
    library: Option<Res<PlantLibrary>>,
    obscurance: Option<ResMut<Obscurance>>,
    fields: Query<(&GrassField, &GlobalTransform)>,
    changed_fields: Query<
        (),
        (
            With<GrassField>,
            Or<(Changed<GrassField>, Changed<GlobalTransform>)>,
        ),
    >,
    plants: Query<(&Plant, &GlobalTransform)>,
    changed_plants: Query<(), (With<Plant>, Or<(Changed<Plant>, Changed<GlobalTransform>)>)>,
    trees: Query<(&SceneryTree, &GlobalTransform)>,
    changed_trees: Query<
        (),
        (
            With<SceneryTree>,
            Or<(Changed<SceneryTree>, Changed<GlobalTransform>)>,
        ),
    >,
    mut gone_fields: RemovedComponents<GrassField>,
    mut gone_plants: RemovedComponents<Plant>,
    mut gone_trees: RemovedComponents<SceneryTree>,
) {
    let Some(mut obscurance) = obscurance else {
        return;
    };
    let b = &mut *built;
    b.fields_dirty |= gone_fields.read().count() > 0 || changed_fields.iter().next().is_some();
    b.trunks_dirty |= gone_plants.read().count() > 0
        || gone_trees.read().count() > 0
        || changed_plants.iter().next().is_some()
        || changed_trees.iter().next().is_some();
    b.wait -= time.delta_secs();
    if !(b.fields_dirty || b.trunks_dirty) || b.wait > 0.0 {
        return;
    }
    b.wait = REBUILD_SECS;
    let started = std::time::Instant::now();
    if std::mem::take(&mut b.fields_dirty) {
        let mut cover = FoliageCover::default();
        for (field, t) in &fields {
            cover.add_field(field, t.affine());
        }
        b.fields = cover.grids;
    }
    if std::mem::take(&mut b.trunks_dirty) {
        let mut cover = FoliageCover::default();
        if let Some(library) = &library {
            let mut trunks = Vec::new();
            for (p, t) in &plants {
                if p.stage.is_alive()
                    && let Some((r, h)) = trunk(library, p.species, p.size, p.genes.height)
                {
                    trunks.push((t.translation(), r, h));
                }
            }
            for (tree, t) in &trees {
                if let Some((r, h)) = trunk(library, tree.species, tree.size, tree.genes.height) {
                    trunks.push((t.translation(), r, h));
                }
            }
            cover.add_trunks(&trunks);
        }
        b.trunks = cover.grids;
    }
    let ms = started.elapsed().as_secs_f32() * 1e3;
    if ms > 2.0 {
        tracing::debug!("foliage: sight cover rebuilt in {ms:.1} ms");
    }
    let grids: Vec<_> = b.fields.iter().chain(&b.trunks).cloned().collect();
    if grids.is_empty() {
        obscurance.remove(SOURCE);
    } else {
        obscurance.set(SOURCE, Arc::new(FoliageCover { grids }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_world::check_obscurance;

    fn meadow(height: f32) -> FoliageCover {
        let mut field = GrassField::flat(20.0, 20.0)
            .with_style(GrassStyle::meadow())
            .sample([21, 21], |x, _| {
                (height, if x < 0.0 { 1.0 } else { 0.0 }, 0.0)
            });
        field.kinds.clear();
        let mut cover = FoliageCover::default();
        cover.add_field(
            &field,
            DAffine3::from_translation(DVec3::new(100.0, 0.0, 0.0)),
        );
        cover
    }

    #[test]
    fn tall_grass_hides_what_lies_in_it() {
        let cover = meadow(2.0);
        let (e, top) = stand_of(&GrassStyle::meadow(), 1.0).unwrap();
        assert!((1.0..3.0).contains(&e), "meadow extinction {e}");
        // In the grass (x < 100), below its top: dim; above it and beside it: clear.
        let low = 2.0 + top as f64 * 0.5;
        assert_eq!(cover.extinction(DVec3::new(95.0, low, 0.0)), e);
        assert_eq!(
            cover.extinction(DVec3::new(95.0, 2.0 + top as f64 + 0.1, 0.0)),
            0.0
        );
        assert_eq!(cover.extinction(DVec3::new(105.0, low, 0.0)), 0.0);
        // A hare 3 m into the grass is hidden from a fox at its height, not from a hawk.
        let hare = DVec3::new(96.0, low, 0.0);
        let fox = DVec3::new(93.0, low, 0.0);
        let hawk = DVec3::new(96.0, 30.0, 0.0);
        assert!(cover.transmittance(fox, hare) < 0.05);
        // (Through the half of the grass above it: about three quarters gets through.)
        assert!(cover.transmittance(hawk, hare) > 0.6);
        // Across the mown half: clear.
        let a = DVec3::new(102.0, low, -5.0);
        let b = DVec3::new(108.0, low, 5.0);
        assert_eq!(cover.transmittance(a, b), 1.0);
    }

    #[test]
    fn foliage_keeps_the_obscurance_contract() {
        let mut cover = meadow(2.0);
        let trunks: Vec<_> = (0..40)
            .map(|i| {
                let p = DVec3::new(
                    -20.0 + (i % 8) as f64 * 5.0,
                    1.0,
                    -10.0 + (i / 8) as f64 * 5.0,
                );
                (p, 0.3, 8.0)
            })
            .collect();
        cover.add_trunks(&trunks);
        let segments = [
            // Through the grass, level, slanting, diagonal and down into it.
            (DVec3::new(89.0, 2.1, 0.3), DVec3::new(99.0, 2.1, 0.7)),
            (DVec3::new(89.0, 2.0, -9.0), DVec3::new(101.0, 2.6, 8.0)),
            (DVec3::new(91.5, 5.0, 2.5), DVec3::new(94.2, 2.05, 2.5)),
            (DVec3::new(99.9, 2.1, 0.0), DVec3::new(100.6, 2.1, 0.0)),
            // Through the wood, and from the wood to the meadow.
            (DVec3::new(-25.0, 2.0, 0.2), DVec3::new(25.0, 3.0, 1.3)),
            (DVec3::new(-10.0, 1.5, -12.0), DVec3::new(95.0, 2.1, 3.0)),
            // Clear air, and a point.
            (DVec3::new(0.0, 50.0, 0.0), DVec3::new(10.0, 60.0, 0.0)),
            (DVec3::new(95.0, 2.1, 0.0), DVec3::new(95.0, 2.1, 0.0)),
        ];
        for (a, b) in segments {
            check_obscurance(&cover, a, b).unwrap_or_else(|e| panic!("{a} -> {b}: {e}"));
        }
    }

    #[test]
    fn a_wood_dims_sight_by_its_trunks() {
        // 0.04 trunks per m² of 0.5 m diameter: a mean free sight line of 50 m.
        let trunks: Vec<_> = (0..400)
            .map(|i| {
                (
                    DVec3::new((i % 20) as f64 * 5.0, 0.0, (i / 20) as f64 * 5.0),
                    0.25,
                    6.0,
                )
            })
            .collect();
        let mut cover = FoliageCover::default();
        cover.add_trunks(&trunks);
        let t = cover.transmittance(DVec3::new(10.0, 1.6, 50.0), DVec3::new(60.0, 1.6, 50.0));
        assert!((0.25..0.5).contains(&t), "50 m through the wood: {t}");
        // Above the boles, clear.
        let up = cover.transmittance(DVec3::new(10.0, 9.0, 50.0), DVec3::new(60.0, 9.0, 50.0));
        assert_eq!(up, 1.0);
    }

    #[test]
    fn litter_hides_nothing() {
        assert!(stand_of(&GrassStyle::leaf_litter(), 1.0).is_none());
        assert!(stand_of(&GrassStyle::fern(), 1.0).is_some());
        let (bramble, _) = stand_of(&GrassStyle::bramble(), 1.0).unwrap();
        assert!(bramble >= 5.0, "bramble {bramble}");
    }
}
