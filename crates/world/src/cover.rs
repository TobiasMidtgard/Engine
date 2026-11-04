//! Ground cover: what grows on the ground around the focus, one meter at a time.
//!
//! Foliage paints its grass and small plants from ecology maps (soil water, shade, slope, wear,
//! fire scars) and writes what it painted into [`GroundCover`], so others can use it without
//! depending on how foliage works: terrain shades meadow ground past the last grass blades from
//! it, footsteps swish through tall grass, movement slows in it and it hides someone crouched,
//! and the weather lays snow on it. Cells outside the grid (or before foliage writes one) are
//! `None`: nothing is known there.

use engine_core::math::{DVec2, DVec3};
use engine_core::prelude::*;

use crate::grid::{Blend, Grid2};

/// What covers the ground in a cell: the grass and low plant communities foliage grows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[repr(u8)]
pub enum CoverKind {
    /// Bare soil, gravel, rock or litter: nothing growing.
    #[default]
    Bare,
    /// Grazed or mown turf, 5-15 cm: paths' verges, around buildings, thin soil.
    Turf,
    /// Meadow grass, 30-50 cm: the base of open fields.
    Meadow,
    /// Tall grass with seed heads, 70-120 cm: moist hollows, field edges, unmown patches.
    TallGrass,
    /// Dry and dead grass, 30-80 cm, straw coloured: sunny slopes, ridges, late summer.
    DryGrass,
    /// Sedge and rush, 40-90 cm, stiff and dark: wet ground, creek and lake margins.
    Sedge,
    /// Clover and broad-leaved forbs, 5-20 cm: patches in turf.
    Forbs,
    /// Bracken, 60-150 cm fronds: forest edges, under birch and pine.
    Bracken,
    /// Heather, 20-50 cm, woody: dry acid ground, pine heath, rocky knolls.
    Heather,
    /// Moss and low berry shrubs: the floor of spruce forest.
    Moss,
}

impl CoverKind {
    pub const ALL: [CoverKind; 10] = [
        CoverKind::Bare,
        CoverKind::Turf,
        CoverKind::Meadow,
        CoverKind::TallGrass,
        CoverKind::DryGrass,
        CoverKind::Sedge,
        CoverKind::Forbs,
        CoverKind::Bracken,
        CoverKind::Heather,
        CoverKind::Moss,
    ];

    /// Lower-case name, for sound and effect keys (`footstep.cover.tall_grass`).
    pub fn name(self) -> &'static str {
        match self {
            CoverKind::Bare => "bare",
            CoverKind::Turf => "turf",
            CoverKind::Meadow => "meadow",
            CoverKind::TallGrass => "tall_grass",
            CoverKind::DryGrass => "dry_grass",
            CoverKind::Sedge => "sedge",
            CoverKind::Forbs => "forbs",
            CoverKind::Bracken => "bracken",
            CoverKind::Heather => "heather",
            CoverKind::Moss => "moss",
        }
    }
}

/// The ground cover in one cell.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct CoverCell {
    /// What grows here most.
    pub kind: CoverKind,
    /// What grows here next most, mixed in at `second_share`.
    pub second: CoverKind,
    /// Share of the cell `second` covers, 0..0.5.
    pub second_share: f32,
    /// Typical height of the plants' tops, meters.
    pub height_m: f32,
    /// 0 green and fresh .. 1 straw dry (paler far shading, fire spreads faster).
    pub dryness: f32,
    /// How much of the ground the plants cover, 0 bare .. 1 a closed sward.
    pub density: f32,
    /// Share of flowering heads among them, 0..1 (for far shading).
    pub flowers: f32,
}

impl Blend for CoverCell {
    fn blend(a: Self, b: Self, t: f32) -> Self {
        let near = if t < 0.5 { a } else { b };
        let mix = |x: f32, y: f32| x + (y - x) * t;
        Self {
            kind: near.kind,
            second: near.second,
            second_share: near.second_share,
            height_m: mix(a.height_m, b.height_m),
            dryness: mix(a.dryness, b.dryness),
            density: mix(a.density, b.density),
            flowers: mix(a.flowers, b.flowers),
        }
    }
}

impl CoverCell {
    /// How much of someone whose eyes are `eye_m` above the ground the cover hides, 0..1:
    /// nothing in turf, most of a crouched figure (eyes ~1 m) in dense tall grass, all of one
    /// lying down. A suggestion for AI sight and stealth; they weigh it as they see fit.
    pub fn hides(&self, eye_m: f32) -> f32 {
        let eye = eye_m.max(0.05);
        let cover = ((self.height_m - 0.4 * eye) / (0.7 * eye)).clamp(0.0, 1.0);
        cover * self.density.clamp(0.0, 1.0)
    }

    /// How hard the cover is to walk through, 0 (turf) .. 1 (dense chest-high bracken or tall
    /// grass): what movement slows by, on its own curve.
    pub fn resistance(&self) -> f32 {
        let tall = ((self.height_m - 0.25) / 0.95).clamp(0.0, 1.0);
        tall * tall * self.density.clamp(0.0, 1.0)
    }
}

/// [`CoverCell`]s on a grid around the focus, written by foliage. Empty until foliage sizes it
/// ([`GroundCover::around`]); recentered by its writer.
#[derive(Resource, Debug, Clone)]
pub struct GroundCover {
    pub grid: Grid2<CoverCell>,
}

impl Default for GroundCover {
    fn default() -> Self {
        Self {
            grid: Grid2::new(DVec2::ZERO, 1.0, 0, 0, CoverCell::default()),
        }
    }
}

impl GroundCover {
    /// A grid `size_m` across of `cell_m` cells centered on `center` (world X, Z), all bare.
    pub fn around(center: DVec2, size_m: f32, cell_m: f32) -> Self {
        Self {
            grid: Grid2::centered(center, size_m, cell_m, CoverCell::default()),
        }
    }

    /// The cover at world `p` (blended between cell centers), `None` outside the grid.
    pub fn sample(&self, p: DVec3) -> Option<CoverCell> {
        self.grid.contains(p).then(|| self.grid.sample(p))
    }

    /// The cell holding world `p`, unblended, `None` outside the grid.
    pub fn cell(&self, p: DVec3) -> Option<CoverCell> {
        self.grid.get(self.grid.cell_at(p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_known_until_foliage_writes() {
        let cover = GroundCover::default();
        assert_eq!(cover.sample(DVec3::ZERO), None);
        assert_eq!(cover.cell(DVec3::new(3.0, 0.0, -2.0)), None);

        let mut cover = GroundCover::around(DVec2::ZERO, 64.0, 1.0);
        let tall = CoverCell {
            kind: CoverKind::TallGrass,
            second: CoverKind::Forbs,
            second_share: 0.2,
            height_m: 1.1,
            dryness: 0.1,
            density: 0.9,
            flowers: 0.3,
        };
        let c = cover.grid.cell_at(DVec3::new(5.5, 0.0, 5.5));
        cover.grid.set(c, tall);
        assert_eq!(cover.cell(DVec3::new(5.2, 0.0, 5.9)), Some(tall));
        assert_eq!(
            cover.sample(DVec3::new(5.5, 0.0, 5.5)).map(|c| c.kind),
            Some(CoverKind::TallGrass)
        );
        assert_eq!(cover.sample(DVec3::new(500.0, 0.0, 0.0)), None);
    }

    #[test]
    fn tall_dense_grass_hides_a_crouched_figure_and_slows_walking() {
        let turf = CoverCell {
            kind: CoverKind::Turf,
            height_m: 0.1,
            density: 1.0,
            ..Default::default()
        };
        let tall = CoverCell {
            kind: CoverKind::TallGrass,
            height_m: 1.1,
            density: 0.9,
            ..Default::default()
        };
        assert_eq!(turf.hides(1.0), 0.0);
        assert!(tall.hides(1.0) > 0.8, "{}", tall.hides(1.0));
        assert!(tall.hides(1.7) < tall.hides(1.0), "standing shows more");
        assert!(turf.resistance() < 0.01 && tall.resistance() > 0.5);
        assert_eq!(CoverKind::TallGrass.name(), "tall_grass");
        assert_eq!(CoverKind::ALL.len(), CoverKind::Moss as usize + 1);
    }
}
