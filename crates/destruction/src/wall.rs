//! Siege-style destructible walls, barricades and reinforced walls.
//!
//! A wall is a grid of small cells across its face. Each cell remembers how much material was
//! removed from the front and from the back, so a bullet leaves a crater, a second bullet in
//! the same spot goes through, and a breach charge blows a ragged hole whose edges taper. Cells
//! that lose their connection to an anchored edge (floor, ceiling, frame) fall out as chunks.
//! Reinforced walls keep their rebar grid in the hole until something cuts it.
//!
//! Local space: the wall stands on its origin, `x` from `-width/2` to `width/2`, `y` from `0`
//! to `height`, thickness along `z` (the front face looks along `+Z`).

use engine_core::math::{Vec2, Vec3};
use engine_core::prelude::*;

use crate::material::{DestructibleMaterial, DestructionMaterials, MaterialProps};
use crate::meshing::{MeshBuilder, hash3, mix, scale};

/// Which edges of a wall are held (by the floor, ceiling, door frame or neighboring walls).
/// Parts that lose their connection to every held edge fall.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WallAnchors {
    pub bottom: bool,
    pub top: bool,
    pub left: bool,
    pub right: bool,
}

impl Default for WallAnchors {
    fn default() -> Self {
        Self::ALL
    }
}

impl WallAnchors {
    /// Held on every edge (a wall between floor, ceiling and two other walls).
    pub const ALL: Self = Self {
        bottom: true,
        top: true,
        left: true,
        right: true,
    };
    /// Free-standing on the ground.
    pub const GROUND: Self = Self {
        bottom: true,
        top: false,
        left: false,
        right: false,
    };
    /// Nailed to a frame at the sides only (barricades).
    pub const SIDES: Self = Self {
        bottom: false,
        top: false,
        left: true,
        right: true,
    };
}

/// A destructible wall, barricade or reinforced panel. Put it on an entity with a
/// `Transform`; the module builds its mesh and collider and keeps them up to date as it
/// breaks. Change any field to rebuild it intact.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[require(Transform)]
#[serde(default)]
pub struct DestructibleWall {
    /// Width (local X) and height (local Y), meters.
    pub size: [f32; 2],
    /// Thickness along local Z, meters.
    pub thickness: f32,
    /// Edge of one damage cell, meters. Smaller = finer holes and more triangles.
    pub cell_size: f32,
    pub material: DestructibleMaterial,
    pub anchors: WallAnchors,
    /// Rebar grid spacing, meters (0 = none). Exposed rebar stays in holes until cut.
    pub rebar_spacing: f32,
    /// Wooden studs inside a soft wall every `stud_spacing` meters (0 = none).
    pub stud_spacing: f32,
    /// Barricade planks: plank height, meters (0 = solid wall).
    pub plank_height: f32,
    /// Gap between planks, meters.
    pub plank_gap: f32,
    /// Masonry block size `[width, height]`, meters, for stone and brick coloring.
    pub masonry: Option<[f32; 2]>,
    /// Linear RGB paint over the material's outside color; `None` = bare material.
    pub paint: Option<[f32; 3]>,
    pub seed: u64,
}

impl Default for DestructibleWall {
    fn default() -> Self {
        Self::soft(3.0, 2.8)
    }
}

impl DestructibleWall {
    /// Drywall over wooden studs: bullets punch through, a breach charge blows a doorway.
    pub fn soft(width: f32, height: f32) -> Self {
        Self {
            size: [width, height],
            thickness: 0.12,
            cell_size: 0.06,
            material: DestructibleMaterial::Plaster,
            anchors: WallAnchors::ALL,
            rebar_spacing: 0.0,
            stud_spacing: 0.4,
            plank_height: 0.0,
            plank_gap: 0.0,
            masonry: None,
            paint: None,
            seed: 1,
        }
    }

    /// Wooden planks nailed across an opening, held at the sides.
    pub fn barricade(width: f32, height: f32) -> Self {
        Self {
            thickness: 0.05,
            cell_size: 0.05,
            material: DestructibleMaterial::Wood,
            anchors: WallAnchors::SIDES,
            stud_spacing: 0.0,
            plank_height: 0.2,
            plank_gap: 0.07,
            ..Self::soft(width, height)
        }
    }

    /// Reinforced concrete with a 20 cm rebar grid: needs a cutting charge to get through.
    pub fn reinforced(width: f32, height: f32) -> Self {
        Self {
            thickness: 0.25,
            cell_size: 0.08,
            material: DestructibleMaterial::ReinforcedConcrete,
            rebar_spacing: 0.2,
            stud_spacing: 0.0,
            ..Self::soft(width, height)
        }
    }

    /// Stone or brick masonry standing on the ground.
    pub fn masonry(
        material: DestructibleMaterial,
        width: f32,
        height: f32,
        thickness: f32,
    ) -> Self {
        // Cells split each course in two so mortar lines fall on cell corners.
        let (block, cell) = match material {
            DestructibleMaterial::Brick => ([0.45, 0.15], 0.075),
            _ => ([0.9, 0.45], 0.15),
        };
        Self {
            thickness,
            cell_size: cell,
            material,
            anchors: WallAnchors {
                bottom: true,
                top: false,
                left: true,
                right: true,
            },
            stud_spacing: 0.0,
            masonry: Some(block),
            ..Self::soft(width, height)
        }
    }

    /// Plain concrete slab wall.
    pub fn concrete(width: f32, height: f32, thickness: f32) -> Self {
        Self {
            thickness,
            cell_size: 0.1,
            material: DestructibleMaterial::Concrete,
            stud_spacing: 0.0,
            ..Self::soft(width, height)
        }
    }

    pub fn with_anchors(mut self, anchors: WallAnchors) -> Self {
        self.anchors = anchors;
        self
    }

    pub fn with_seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }

    pub fn with_paint(mut self, paint: [f32; 3]) -> Self {
        self.paint = Some(paint);
        self
    }
}

/// What a cell is made of inside the wall.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CellKind {
    #[default]
    Main,
    /// A wooden stud inside a soft wall.
    Stud,
}

/// Rebar bits per cell.
pub const VBAR: u8 = 1;
pub const HBAR: u8 = 2;

/// The damage grid of a wall (or of a chunk that broke off one).
#[derive(Debug, Clone, Default)]
pub struct CellGrid {
    pub nx: usize,
    pub ny: usize,
    /// Cell size (x, y), meters.
    pub cell: Vec2,
    /// Local XY of cell (0, 0)'s lower-left corner.
    pub origin: Vec2,
    pub thickness: f32,
    /// Material removed from the front (+Z) face, meters.
    pub front: Vec<f32>,
    /// Material removed from the back (−Z) face, meters.
    pub back: Vec<f32>,
    pub solid: Vec<bool>,
    pub kind: Vec<CellKind>,
    /// Toughness multiplier (material variation, studs).
    pub tough: Vec<f32>,
    /// Blast scorch 0..1.
    pub scorch: Vec<f32>,
    /// Intact rebar through this cell (`VBAR`, `HBAR`).
    pub rebar: Vec<u8>,
    /// Index of cell (0, 0) in the wall this grid came from (stable noise and jitter).
    pub global: (i64, i64),
}

impl CellGrid {
    pub fn len(&self) -> usize {
        self.nx * self.ny
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn idx(&self, i: usize, j: usize) -> usize {
        j * self.nx + i
    }

    pub fn coords(&self, idx: usize) -> (usize, usize) {
        (idx % self.nx, idx / self.nx)
    }

    /// Local XY of a cell's center.
    pub fn center(&self, idx: usize) -> Vec2 {
        let (i, j) = self.coords(idx);
        self.origin
            + Vec2::new(
                (i as f32 + 0.5) * self.cell.x,
                (j as f32 + 0.5) * self.cell.y,
            )
    }

    /// Cell containing local XY, if inside the grid.
    pub fn cell_at(&self, p: Vec2) -> Option<usize> {
        let q = (p - self.origin) / self.cell;
        if q.x < 0.0 || q.y < 0.0 {
            return None;
        }
        let (i, j) = (q.x as usize, q.y as usize);
        (i < self.nx && j < self.ny).then(|| self.idx(i, j))
    }

    /// Material left in a cell, meters.
    pub fn remaining(&self, idx: usize) -> f32 {
        if self.solid[idx] {
            (self.thickness - self.front[idx] - self.back[idx]).max(0.0)
        } else {
            0.0
        }
    }

    pub fn solid_count(&self) -> usize {
        self.solid.iter().filter(|s| **s).count()
    }

    /// Volume of material left, m³.
    pub fn volume(&self) -> f32 {
        let area = self.cell.x * self.cell.y;
        (0..self.len()).map(|i| self.remaining(i)).sum::<f32>() * area
    }

    /// Remove `depth` meters from one face of a cell. Returns `true` if this destroyed it.
    pub fn erode(&mut self, idx: usize, depth: f32, front: bool) -> bool {
        if !self.solid[idx] || depth <= 0.0 {
            return false;
        }
        if front {
            self.front[idx] += depth;
        } else {
            self.back[idx] += depth;
        }
        if self.front[idx] + self.back[idx] >= self.thickness * 0.985 {
            self.solid[idx] = false;
            true
        } else {
            false
        }
    }

    /// Cell rectangle `(i0, j0, i1, j1)` (exclusive end) covering local XY within `r` of `p`.
    pub fn cells_near(&self, p: Vec2, r: f32) -> (usize, usize, usize, usize) {
        let lo = ((p - Vec2::splat(r) - self.origin) / self.cell).floor();
        let hi = ((p + Vec2::splat(r) - self.origin) / self.cell).ceil();
        let clamp = |v: f32, n: usize| (v.max(0.0) as usize).min(n);
        (
            clamp(lo.x, self.nx),
            clamp(lo.y, self.ny),
            clamp(hi.x, self.nx),
            clamp(hi.y, self.ny),
        )
    }
}

/// A destroyed cell, with the material it had just before (for debris).
#[derive(Debug, Clone, Copy)]
pub struct LostCell {
    pub idx: usize,
    pub front: f32,
    pub back: f32,
}

/// What a hit did to a wall.
#[derive(Debug, Clone, Default)]
pub struct WallHit {
    /// Cells destroyed by this hit.
    pub lost: Vec<LostCell>,
    /// Material removed in total (craters and holes), m³.
    pub volume: f32,
    /// A bullet went through: where it came out (local).
    pub exit: Option<Vec3>,
    /// Damaged cell rectangle `(i0, j0, i1, j1)`, for remeshing.
    pub rect: Option<(usize, usize, usize, usize)>,
}

impl WallHit {
    fn touch(&mut self, i: usize, j: usize) {
        self.rect = Some(match self.rect {
            None => (i, j, i + 1, j + 1),
            Some((a, b, c, d)) => (a.min(i), b.min(j), c.max(i + 1), d.max(j + 1)),
        });
    }
}

/// Runtime state of a [`DestructibleWall`] (managed by the module).
#[derive(Component, Debug, Clone, Default)]
pub struct WallState {
    pub grid: CellGrid,
    pub material: DestructibleMaterial,
    pub anchors: WallAnchors,
    pub initial_volume: f32,
    /// Mesh tiles: `tiles_x × tiles_y` child entities, each `TILE` cells square.
    pub tiles_x: usize,
    pub tiles_y: usize,
    pub tile_entities: Vec<Option<Entity>>,
    pub tile_dirty: Vec<bool>,
    pub collider_dirty: bool,
    /// Rebar position offsets so bars line up across the wall.
    pub rebar_spacing: f32,
}

/// Cells per mesh tile edge.
pub const TILE: usize = 24;

impl WallState {
    /// Fresh, intact state for `wall`.
    pub fn new(wall: &DestructibleWall, materials: &DestructionMaterials) -> Self {
        let [w, h] = wall.size;
        let cs = wall.cell_size.max(0.01);
        let nx = ((w / cs).round() as usize).clamp(1, 4096);
        let ny = ((h / cs).round() as usize).clamp(1, 4096);
        let cell = Vec2::new(w / nx as f32, h / ny as f32);
        let n = nx * ny;
        let props = materials.get(wall.material);
        let wood = materials.get(DestructibleMaterial::Wood);
        let mut grid = CellGrid {
            nx,
            ny,
            cell,
            origin: Vec2::new(-w * 0.5, 0.0),
            thickness: wall.thickness.max(0.01),
            front: vec![0.0; n],
            back: vec![0.0; n],
            solid: vec![true; n],
            kind: vec![CellKind::Main; n],
            tough: vec![1.0; n],
            scorch: vec![0.0; n],
            rebar: vec![0; n],
            global: (0, 0),
        };
        for idx in 0..n {
            let (i, j) = grid.coords(idx);
            let c = grid.center(idx);
            // Correlated variation: neighbors are similar, so breaks follow weak zones.
            let noise =
                engine_core::math::noise::perlin2(c.x as f64 * 3.1, c.y as f64 * 3.1, wall.seed)
                    * 0.7
                    + (hash3(i as i64, j as i64, 7, wall.seed) - 0.5) * 0.6;
            grid.tough[idx] = (1.0 + noise * props.irregularity).max(0.3);
            if wall.stud_spacing > 0.0 {
                let x = c.x - grid.origin.x;
                let phase = x.rem_euclid(wall.stud_spacing);
                if phase < 0.09_f32.max(cell.x) {
                    grid.kind[idx] = CellKind::Stud;
                    grid.tough[idx] *= wood.toughness_j_m3 / props.toughness_j_m3;
                }
            }
            if wall.plank_height > 0.0 {
                let pitch = wall.plank_height + wall.plank_gap;
                let y = c.y.rem_euclid(pitch);
                if y > wall.plank_height {
                    grid.solid[idx] = false;
                }
            }
        }
        if wall.rebar_spacing > 0.0 {
            let s = wall.rebar_spacing;
            for idx in 0..n {
                let (i, j) = grid.coords(idx);
                let x0 = i as f32 * cell.x;
                let y0 = j as f32 * cell.y;
                // A bar runs through this cell if a bar line falls inside it.
                let bar_in = |lo: f32, size: f32| {
                    let k = ((lo + size) / s - 0.5).floor();
                    let line = (k + 0.5) * s;
                    line >= lo && line < lo + size
                };
                let mut bits = 0;
                if bar_in(x0, cell.x) {
                    bits |= VBAR;
                }
                if bar_in(y0, cell.y) {
                    bits |= HBAR;
                }
                grid.rebar[idx] = bits;
            }
        }
        let tiles_x = nx.div_ceil(TILE);
        let tiles_y = ny.div_ceil(TILE);
        let initial_volume = grid.volume();
        Self {
            grid,
            material: wall.material,
            anchors: wall.anchors,
            initial_volume,
            tiles_x,
            tiles_y,
            tile_entities: vec![None; tiles_x * tiles_y],
            tile_dirty: vec![true; tiles_x * tiles_y],
            collider_dirty: true,
            rebar_spacing: wall.rebar_spacing,
        }
    }

    /// Fraction of the original material left, 0..1.
    pub fn integrity(&self) -> f32 {
        if self.initial_volume <= 0.0 {
            return 0.0;
        }
        (self.grid.volume() / self.initial_volume).clamp(0.0, 1.0)
    }

    /// Mark the tiles overlapping a cell rectangle (plus one cell around it) for remeshing.
    pub fn mark_dirty(&mut self, rect: (usize, usize, usize, usize)) {
        let (i0, j0, i1, j1) = rect;
        let tx0 = i0.saturating_sub(1) / TILE;
        let ty0 = j0.saturating_sub(1) / TILE;
        let tx1 = ((i1 + 1).min(self.grid.nx) - 1) / TILE;
        let ty1 = ((j1 + 1).min(self.grid.ny) - 1) / TILE;
        for ty in ty0..=ty1.min(self.tiles_y - 1) {
            for tx in tx0..=tx1.min(self.tiles_x - 1) {
                self.tile_dirty[ty * self.tiles_x + tx] = true;
            }
        }
        self.collider_dirty = true;
    }

    /// A bullet, blade or blunt impact. `point` and `dir` are local; `energy_j` is the energy
    /// that goes into breaking material. `min_radius` keeps holes at least caliber-sized.
    pub fn crater(
        &mut self,
        props: &MaterialProps,
        point: Vec3,
        dir: Vec3,
        energy_j: f32,
        min_radius: f32,
        aspect_scale: f32,
    ) -> WallHit {
        let t = self.grid.thickness;
        let dir = dir.normalize_or(Vec3::NEG_Z);
        let front = if dir.z.abs() > 0.05 {
            dir.z < 0.0
        } else {
            point.z >= 0.0
        };
        let face_z = if front { t * 0.5 } else { -t * 0.5 };
        let s = if dir.z.abs() > 1e-3 {
            (face_z - point.z) / dir.z
        } else {
            0.0
        };
        let hit = point + dir * s;
        let p = Vec2::new(hit.x, hit.y);
        let mut out = WallHit::default();
        let Some(center) = self.grid.cell_at(p) else {
            return out;
        };
        if !self.grid.solid[center] {
            // Through an existing hole.
            out.exit = Some(Vec3::new(p.x, p.y, -face_z));
            return out;
        }
        let aspect = (props.crater_aspect * aspect_scale).max(0.05);
        let volume = energy_j.max(0.0) / props.toughness_j_m3;
        let r = ((2.0 * volume / (std::f32::consts::PI * aspect)).cbrt()).clamp(min_radius, 2.0);
        let h = aspect * r;
        let remaining = self.grid.remaining(center);
        self.apply_crater(p, r, h, front, &mut out);
        let center_depth = h / self.grid.tough[center].max(0.1);
        if center_depth > remaining {
            // Through: spall a wider, shallower crater out of the far face.
            let leftover = center_depth - remaining;
            let r2 = r * 1.4;
            let h2 = (leftover * 0.4 + h * 0.25).min(t);
            self.apply_crater(p, r2, h2, !front, &mut out);
            out.exit = Some(Vec3::new(p.x, p.y, -face_z));
        }
        out
    }

    fn apply_crater(&mut self, p: Vec2, r: f32, h: f32, front: bool, out: &mut WallHit) {
        let g = &mut self.grid;
        // Always hit the cell under the impact, even when the crater is smaller than a cell.
        let r_eff = r.max(g.cell.length() * 0.5);
        let (i0, j0, i1, j1) = g.cells_near(p, r_eff);
        let area = g.cell.x * g.cell.y;
        let under = g.cell_at(p);
        for j in j0..j1 {
            for i in i0..i1 {
                let idx = g.idx(i, j);
                if !g.solid[idx] {
                    continue;
                }
                let d = g.center(idx).distance(p);
                let mut w = 1.0 - (d / r_eff).powi(2);
                if Some(idx) == under {
                    w = w.max(0.7);
                }
                if w <= 0.0 {
                    continue;
                }
                let depth = h * w / g.tough[idx].max(0.1);
                let (f0, b0) = (g.front[idx], g.back[idx]);
                let before = g.remaining(idx);
                let gone = g.erode(idx, depth, front);
                out.volume += (before - g.remaining(idx)) * area;
                out.touch(i, j);
                if gone {
                    out.lost.push(LostCell {
                        idx,
                        front: f0,
                        back: b0,
                    });
                }
            }
        }
    }

    /// An explosion at local `center` releasing `energy_j`. Material within reach is blown
    /// away (depth = flux / toughness); rebar is cut where the flux exceeds `rebar_cut_flux`.
    pub fn blast(
        &mut self,
        props: &MaterialProps,
        center: Vec3,
        energy_j: f32,
        rebar_cut_flux: f32,
    ) -> WallHit {
        let mut out = WallHit::default();
        let g = &mut self.grid;
        let t = g.thickness;
        // Farthest distance where the blast still scorches (flux 2e4 J/m²).
        let reach = (energy_j / (4.0 * std::f32::consts::PI * 2.0e4))
            .sqrt()
            .min(60.0);
        let p = Vec2::new(center.x, center.y);
        let lateral = (reach * reach - (center.z.abs() - t * 0.5).max(0.0).powi(2))
            .max(0.0)
            .sqrt();
        if lateral <= 0.0 {
            return out;
        }
        let (i0, j0, i1, j1) = g.cells_near(p, lateral);
        let area = g.cell.x * g.cell.y;
        let front = center.z >= 0.0;
        for j in j0..j1 {
            for i in i0..i1 {
                let idx = g.idx(i, j);
                let c = g.center(idx);
                let surface_z = if front {
                    t * 0.5 - g.front[idx]
                } else {
                    -t * 0.5 + g.back[idx]
                };
                let offset = Vec3::new(c.x - center.x, c.y - center.y, surface_z - center.z);
                let d = offset.length().max(0.06);
                // Contact charges shatter sideways through the material too, not only straight ahead.
                let facing = 0.5 + 0.5 * (surface_z - center.z).abs() / d;
                let flux = energy_j / (4.0 * std::f32::consts::PI * d * d);
                if flux >= rebar_cut_flux {
                    g.rebar[idx] = 0;
                }
                if !g.solid[idx] {
                    continue;
                }
                g.scorch[idx] = (g.scorch[idx] + flux / 3.0e5).min(1.0);
                let depth = flux * facing / (props.toughness_j_m3 * g.tough[idx].max(0.1));
                if depth < 1e-4 {
                    out.touch(i, j);
                    continue;
                }
                let (f0, b0) = (g.front[idx], g.back[idx]);
                let before = g.remaining(idx);
                let mut gone = g.erode(idx, depth, front);
                // The shock spalls the far face of cells it nearly went through.
                if !gone && depth > before * 0.55 {
                    gone = g.erode(idx, depth * 0.35, !front);
                }
                out.volume += (before - g.remaining(idx)) * area;
                out.touch(i, j);
                if gone {
                    out.lost.push(LostCell {
                        idx,
                        front: f0,
                        back: b0,
                    });
                }
            }
        }
        out
    }

    /// Build an opening (a door or window frame) into the wall: cells whose centers are inside
    /// `[min, max]` (local XY) are removed with no scorch or debris.
    pub fn open(&mut self, min: Vec2, max: Vec2) {
        let g = &mut self.grid;
        for idx in 0..g.nx * g.ny {
            let c = g.center(idx);
            if c.x >= min.x && c.x <= max.x && c.y >= min.y && c.y <= max.y {
                g.solid[idx] = false;
                g.rebar[idx] = 0;
                g.front[idx] = 0.0;
                g.back[idx] = 0.0;
            }
        }
        self.initial_volume = g.volume();
        let (nx, ny) = (g.nx, g.ny);
        self.mark_dirty((0, 0, nx, ny));
    }

    /// Cut a rectangle out cleanly (thermite, a cutting torch): every cell whose center is
    /// inside `[min, max]` (local XY) is removed, rebar included. The cut-out cells come back
    /// in `lost` so they can fall as one slab.
    pub fn cut_rect(&mut self, min: Vec2, max: Vec2) -> WallHit {
        let mut out = WallHit::default();
        let g = &mut self.grid;
        let mid = (min + max) * 0.5;
        let half = (max - min) * 0.5;
        let (i0, j0, i1, j1) = g.cells_near(mid, half.max_element() + g.cell.max_element());
        let area = g.cell.x * g.cell.y;
        for j in j0..j1 {
            for i in i0..i1 {
                let idx = g.idx(i, j);
                let c = g.center(idx);
                let inside = c.x >= min.x && c.x <= max.x && c.y >= min.y && c.y <= max.y;
                let ring = (c.x >= min.x - g.cell.x * 2.0
                    && c.x <= max.x + g.cell.x * 2.0
                    && c.y >= min.y - g.cell.y * 2.0
                    && c.y <= max.y + g.cell.y * 2.0)
                    && !inside;
                if ring {
                    g.scorch[idx] = 1.0;
                    out.touch(i, j);
                }
                if !inside {
                    continue;
                }
                g.rebar[idx] = 0;
                out.touch(i, j);
                if g.solid[idx] {
                    out.volume += g.remaining(idx) * area;
                    out.lost.push(LostCell {
                        idx,
                        front: g.front[idx],
                        back: g.back[idx],
                    });
                    g.solid[idx] = false;
                    g.front[idx] = 0.0;
                    g.back[idx] = 0.0;
                }
            }
        }
        out
    }

    /// Find solid cells no longer connected to an anchored edge, remove them from the wall
    /// and return them grouped into connected islands. Rebar holds cells too: a chunk hanging
    /// on intact bars stays. Bars left dangling with nothing to hold them are removed.
    pub fn detach_unsupported(&mut self) -> Vec<Vec<LostCell>> {
        let g = &mut self.grid;
        let (nx, ny) = (g.nx, g.ny);
        let n = nx * ny;
        let conducts_v = |g: &CellGrid, i: usize| g.solid[i] || g.rebar[i] & VBAR != 0;
        let conducts_h = |g: &CellGrid, i: usize| g.solid[i] || g.rebar[i] & HBAR != 0;
        let mut reached = vec![false; n];
        let mut stack = Vec::new();
        let seed = |idx: usize, ok: bool, reached: &mut Vec<bool>, stack: &mut Vec<usize>| {
            if ok && !reached[idx] {
                reached[idx] = true;
                stack.push(idx);
            }
        };
        if self.anchors.bottom {
            for i in 0..nx {
                let idx = g.idx(i, 0);
                seed(idx, conducts_v(g, idx), &mut reached, &mut stack);
            }
        }
        if self.anchors.top {
            for i in 0..nx {
                let idx = g.idx(i, ny - 1);
                seed(idx, conducts_v(g, idx), &mut reached, &mut stack);
            }
        }
        if self.anchors.left {
            for j in 0..ny {
                let idx = g.idx(0, j);
                seed(idx, conducts_h(g, idx), &mut reached, &mut stack);
            }
        }
        if self.anchors.right {
            for j in 0..ny {
                let idx = g.idx(nx - 1, j);
                seed(idx, conducts_h(g, idx), &mut reached, &mut stack);
            }
        }
        while let Some(idx) = stack.pop() {
            let (i, j) = g.coords(idx);
            let mut visit = |k: usize, vertical: bool| {
                if reached[k] {
                    return;
                }
                let ok = if vertical {
                    conducts_v(g, idx) && conducts_v(g, k)
                } else {
                    conducts_h(g, idx) && conducts_h(g, k)
                };
                if ok {
                    reached[k] = true;
                    stack.push(k);
                }
            };
            if i > 0 {
                visit(idx - 1, false);
            }
            if i + 1 < nx {
                visit(idx + 1, false);
            }
            if j > 0 {
                visit(idx - nx, true);
            }
            if j + 1 < ny {
                visit(idx + nx, true);
            }
        }
        // Dangling rebar with nothing holding it falls away with the concrete.
        for (idx, reached) in reached.iter().enumerate() {
            if !reached && !g.solid[idx] {
                g.rebar[idx] = 0;
            }
        }
        let mut islands = Vec::new();
        let mut taken = vec![false; n];
        for start in 0..n {
            if reached[start] || !g.solid[start] || taken[start] {
                continue;
            }
            let mut island = Vec::new();
            taken[start] = true;
            stack.push(start);
            while let Some(idx) = stack.pop() {
                island.push(LostCell {
                    idx,
                    front: g.front[idx],
                    back: g.back[idx],
                });
                let (i, j) = g.coords(idx);
                let mut next = |k: usize| {
                    if g.solid[k] && !reached[k] && !taken[k] {
                        taken[k] = true;
                        stack.push(k);
                    }
                };
                if i > 0 {
                    next(idx - 1);
                }
                if i + 1 < nx {
                    next(idx + 1);
                }
                if j > 0 {
                    next(idx - nx);
                }
                if j + 1 < ny {
                    next(idx + nx);
                }
            }
            for c in &island {
                g.solid[c.idx] = false;
                g.front[c.idx] = 0.0;
                g.back[c.idx] = 0.0;
                g.rebar[c.idx] = 0;
            }
            islands.push(island);
        }
        if !islands.is_empty() {
            self.mark_dirty((0, 0, nx, ny));
        }
        islands
    }

    /// Sub-grid holding only `cells` (as they were before they broke off), for a debris
    /// chunk's mesh. Its `origin` places it in the wall's local space.
    pub fn chunk_grid(&self, cells: &[LostCell]) -> CellGrid {
        let g = &self.grid;
        let (mut i0, mut j0, mut i1, mut j1) = (usize::MAX, usize::MAX, 0, 0);
        for c in cells {
            let (i, j) = g.coords(c.idx);
            i0 = i0.min(i);
            j0 = j0.min(j);
            i1 = i1.max(i + 1);
            j1 = j1.max(j + 1);
        }
        let (nx, ny) = (i1 - i0, j1 - j0);
        let n = nx * ny;
        let mut out = CellGrid {
            nx,
            ny,
            cell: g.cell,
            origin: g.origin + Vec2::new(i0 as f32 * g.cell.x, j0 as f32 * g.cell.y),
            thickness: g.thickness,
            front: vec![0.0; n],
            back: vec![0.0; n],
            solid: vec![false; n],
            kind: vec![CellKind::Main; n],
            tough: vec![1.0; n],
            scorch: vec![0.0; n],
            rebar: vec![0; n],
            global: (g.global.0 + i0 as i64, g.global.1 + j0 as i64),
        };
        for c in cells {
            let (i, j) = g.coords(c.idx);
            let k = out.idx(i - i0, j - j0);
            let t = g.thickness;
            // Keep a sliver so eroded cells still have some body.
            let total = c.front + c.back;
            let (f, b) = if total > t * 0.9 {
                let s = t * 0.9 / total.max(1e-6);
                (c.front * s, c.back * s)
            } else {
                (c.front, c.back)
            };
            out.front[k] = f;
            out.back[k] = b;
            out.solid[k] = true;
            out.kind[k] = g.kind[c.idx];
            out.tough[k] = g.tough[c.idx];
            out.scorch[k] = g.scorch[c.idx];
        }
        out
    }
}

/// Colors and style for meshing a [`CellGrid`].
#[derive(Debug, Clone)]
pub struct WallLook {
    pub exterior: [f32; 3],
    pub interior: [f32; 3],
    pub stud_interior: [f32; 3],
    pub rebar: [f32; 3],
    pub masonry: Option<[f32; 2]>,
    /// Plank pitch (height + gap) for per-plank wood tint.
    pub plank_pitch: Option<f32>,
    pub seed: u64,
    /// Keep the grid's outer border straight (true for walls, false for broken chunks).
    pub straight_border: bool,
    /// The material's own exterior color (what its texture already shows).
    pub reference: [f32; 3],
    /// How readily cracks run out from damage ([`crate::cracks::brittleness`]).
    pub cracks: f32,
}

impl WallLook {
    pub fn new(wall: &DestructibleWall, materials: &DestructionMaterials) -> Self {
        let props = materials.get(wall.material);
        let wood = materials.get(DestructibleMaterial::Wood);
        Self {
            exterior: wall.paint.unwrap_or(props.exterior),
            interior: props.interior,
            stud_interior: wood.interior,
            rebar: materials.rebar_color,
            masonry: wall.masonry,
            plank_pitch: (wall.plank_height > 0.0).then_some(wall.plank_height + wall.plank_gap),
            seed: wall.seed,
            straight_border: true,
            reference: props.exterior,
            cracks: crate::cracks::brittleness(wall.material),
        }
    }

    /// Textured surfaces carry their own albedo, bricks and blocks: colors become relative to
    /// the material's own exterior color (paint and fresh breaks still show) and the
    /// vertex-color masonry goes.
    pub fn with_texture(mut self, textured: bool) -> Self {
        if textured {
            let r = self.reference;
            let rel =
                |c: [f32; 3]| std::array::from_fn(|k| (c[k] / r[k].max(1e-3)).clamp(0.3, 1.35));
            self.exterior = rel(self.exterior);
            self.interior = rel(self.interior);
            self.stud_interior = rel(self.stud_interior);
            self.rebar = rel(self.rebar);
            self.masonry = None;
        }
        self
    }

    fn outside_color(&self, p: Vec2, gi: i64, gj: i64) -> [f32; 3] {
        let mut c = self.exterior;
        let grain = hash3(gi, gj, 11, self.seed);
        if let Some([bw, bh]) = self.masonry {
            let row = (p.y / bh).floor();
            let offset = if (row as i64).rem_euclid(2) == 1 {
                bw * 0.5
            } else {
                0.0
            };
            let col = ((p.x + offset) / bw).floor();
            let tint = 0.82 + 0.3 * hash3(col as i64, row as i64, 3, self.seed);
            let fx = ((p.x + offset) / bw).fract().abs();
            let fy = (p.y / bh).fract().abs();
            let edge = (fx.min(1.0 - fx) * bw).min(fy.min(1.0 - fy) * bh);
            let mortar = if edge < 0.01 { 0.62 } else { 1.0 };
            c = scale(c, tint * mortar);
        } else if let Some(pitch) = self.plank_pitch {
            let plank = (p.y / pitch).floor();
            let tint = 0.8 + 0.35 * hash3(plank as i64, 0, 5, self.seed);
            let streak = 0.93 + 0.07 * ((p.x * 9.0 + plank * 2.3).sin() * (p.x * 2.1).cos());
            c = scale(c, tint * streak);
        }
        scale(c, 0.95 + 0.1 * grain)
    }
}

/// Per-vertex data for meshing.
#[derive(Clone, Copy)]
struct GridVertex {
    xy: Vec2,
    zf: f32,
    zb: f32,
    removed_front: f32,
    removed_back: f32,
    scorch: f32,
    stud: bool,
}

/// Mesh of the cells in `[i0, i1) × [j0, j1)` of `grid` (local wall space, plus `offset`).
/// Front and back faces are smooth so craters read as bowls; broken edges get the interior
/// color; exposed rebar is drawn as thin rods.
pub fn mesh_cells(
    grid: &CellGrid,
    look: &WallLook,
    rect: (usize, usize, usize, usize),
    offset: Vec3,
) -> MeshBuilder {
    let (i0, j0, i1, j1) = rect;
    let (nx, ny) = (grid.nx, grid.ny);
    let t = grid.thickness;
    let vw = i1 - i0 + 1;
    let vh = j1 - j0 + 1;
    let mut verts: Vec<Option<GridVertex>> = vec![None; vw * vh];
    for vj in j0..=j1 {
        for vi in i0..=i1 {
            let mut f = 0.0;
            let mut b = 0.0;
            let mut sc = 0.0;
            let mut count = 0.0;
            let mut any_solid = false;
            let mut damaged = false;
            let mut stud = false;
            for (di, dj) in [(0usize, 0usize), (1, 0), (0, 1), (1, 1)] {
                if vi < di || vj < dj {
                    continue;
                }
                let (ci, cj) = (vi - di, vj - dj);
                if ci >= nx || cj >= ny {
                    continue;
                }
                let c = grid.idx(ci, cj);
                count += 1.0;
                if grid.solid[c] {
                    any_solid = true;
                    f += grid.front[c];
                    b += grid.back[c];
                    if grid.front[c] > 1e-3 || grid.back[c] > 1e-3 {
                        damaged = true;
                    }
                } else {
                    damaged = true;
                    f += grid.front[c].min(t);
                    b += grid.back[c].min(t);
                }
                sc += grid.scorch[c];
                stud |= grid.kind[c] == CellKind::Stud;
            }
            if !any_solid {
                continue;
            }
            f /= count;
            b /= count;
            sc /= count;
            let max_sum = t * 0.92;
            if f + b > max_sum {
                let s = max_sum / (f + b);
                f *= s;
                b *= s;
            }
            let gx = grid.global.0 + vi as i64;
            let gy = grid.global.1 + vj as i64;
            let mut xy = grid.origin + Vec2::new(vi as f32 * grid.cell.x, vj as f32 * grid.cell.y);
            let on_border = vi == 0 || vj == 0 || vi == nx || vj == ny;
            if damaged && !(look.straight_border && on_border) {
                let jx = hash3(gx, gy, 1, look.seed) - 0.5;
                let jy = hash3(gx, gy, 2, look.seed) - 0.5;
                xy += Vec2::new(jx * grid.cell.x, jy * grid.cell.y) * 0.75;
            }
            verts[(vj - j0) * vw + (vi - i0)] = Some(GridVertex {
                xy,
                zf: t * 0.5 - f,
                zb: -t * 0.5 + b,
                removed_front: f,
                removed_back: b,
                scorch: sc,
                stud,
            });
        }
    }
    let vert = |vi: usize, vj: usize| verts[(vj - j0) * vw + (vi - i0)];

    let mut mb = MeshBuilder::new();
    let interior_of = |stud: bool| {
        if stud {
            look.stud_interior
        } else {
            look.interior
        }
    };
    let surface_color = |v: &GridVertex, removed: f32, gx: i64, gy: i64| {
        let base = scale(look.outside_color(v.xy, gx, gy), 1.0 - 0.8 * v.scorch);
        let k = ((removed - 0.002) / 0.02).clamp(0.0, 1.0);
        let inner = scale(interior_of(v.stud), 1.0 - 0.5 * v.scorch);
        mix(base, scale(inner, 0.93), k)
    };

    // Front and back faces with shared, smoothed vertices.
    for front in [true, false] {
        let vstart = mb.vertices.len();
        let tstart = mb.indices.len();
        let mut index = vec![u32::MAX; vw * vh];
        for j in j0..j1 {
            for i in i0..i1 {
                let c = grid.idx(i, j);
                if !grid.solid[c] {
                    continue;
                }
                let corners = [(i, j), (i + 1, j), (i + 1, j + 1), (i, j + 1)];
                let mut ids = [0u32; 4];
                for (k, &(vi, vj)) in corners.iter().enumerate() {
                    let slot = (vj - j0) * vw + (vi - i0);
                    if index[slot] == u32::MAX {
                        let v = vert(vi, vj).expect("corner of a solid cell");
                        let z = if front { v.zf } else { v.zb };
                        let removed = if front {
                            v.removed_front
                        } else {
                            v.removed_back
                        };
                        let gx = grid.global.0 + vi as i64;
                        let gy = grid.global.1 + vj as i64;
                        let color = surface_color(&v, removed, gx, gy);
                        let n = if front { Vec3::Z } else { Vec3::NEG_Z };
                        index[slot] = mb.vertex(Vec3::new(v.xy.x, v.xy.y, z), n, color);
                    }
                    ids[k] = index[slot];
                }
                if front {
                    mb.triangle(ids[0], ids[1], ids[2]);
                    mb.triangle(ids[0], ids[2], ids[3]);
                } else {
                    mb.triangle(ids[0], ids[2], ids[1]);
                    mb.triangle(ids[0], ids[3], ids[2]);
                }
            }
        }
        let vend = mb.vertices.len();
        let tend = mb.indices.len();
        mb.smooth_normals(vstart..vend, tstart..tend);
    }

    // Edges: wherever a solid cell meets a hole or the wall's border.
    for j in j0..j1 {
        for i in i0..i1 {
            let c = grid.idx(i, j);
            if !grid.solid[c] {
                continue;
            }
            let stud = grid.kind[c] == CellKind::Stud;
            let sides: [((usize, usize), (usize, usize), Option<usize>, Vec3); 4] = [
                (
                    (i + 1, j),
                    (i + 1, j + 1),
                    (i + 1 < nx).then(|| c + 1),
                    Vec3::X,
                ),
                ((i, j + 1), (i, j), (i > 0).then(|| c - 1), Vec3::NEG_X),
                (
                    (i + 1, j + 1),
                    (i, j + 1),
                    (j + 1 < ny).then(|| c + nx),
                    Vec3::Y,
                ),
                ((i, j), (i + 1, j), (j > 0).then(|| c - nx), Vec3::NEG_Y),
            ];
            for (a, b, neighbor, out) in sides {
                let open = match neighbor {
                    Some(k) => !grid.solid[k],
                    None => true,
                };
                if !open {
                    continue;
                }
                let (Some(va), Some(vb)) = (vert(a.0, a.1), vert(b.0, b.1)) else {
                    continue;
                };
                let broken = neighbor.is_some() || !look.straight_border;
                let color = if broken {
                    scale(interior_of(stud), 0.86)
                } else {
                    scale(look.exterior, 0.8)
                };
                let pa = Vec3::new(va.xy.x, va.xy.y, 0.0);
                let pb = Vec3::new(vb.xy.x, vb.xy.y, 0.0);
                let (fa, fb) = (pa + Vec3::Z * va.zf, pb + Vec3::Z * vb.zf);
                let (ba, bb) = (pa + Vec3::Z * va.zb, pb + Vec3::Z * vb.zb);
                if !broken {
                    mb.quad_facing([fa, ba, bb, fb], out, color);
                    continue;
                }
                // A fracture isn't a clean extrusion: break the edge at a ridge partway
                // through, jittered per grid vertex so neighboring edges share it.
                let ridge = |v: &GridVertex, p: Vec3, vi: usize, vj: usize| {
                    let (gx, gy) = (grid.global.0 + vi as i64, grid.global.1 + vj as i64);
                    let jx = hash3(gx, gy, 31, look.seed) - 0.5;
                    let jy = hash3(gx, gy, 32, look.seed) - 0.5;
                    let jz = hash3(gx, gy, 33, look.seed) - 0.5;
                    let mid = 0.5 * (v.zf + v.zb) + jz * 0.5 * (v.zf - v.zb);
                    p + Vec3::new(jx * grid.cell.x, jy * grid.cell.y, 0.0) * 0.8 + Vec3::Z * mid
                };
                let ma = ridge(&va, pa, a.0, a.1);
                let mbv = ridge(&vb, pb, b.0, b.1);
                let lit = scale(color, 1.08);
                mb.quad_facing([fa, ma, mbv, fb], out + Vec3::Z * 0.3, lit);
                mb.quad_facing([ma, ba, bb, mbv], out - Vec3::Z * 0.3, color);
            }
        }
    }

    // Hairline cracks out from holes and craters across the intact faces.
    for front in [true, false] {
        let z = if front {
            t * 0.5 + 0.0012
        } else {
            -t * 0.5 - 0.0012
        };
        let n = if front { Vec3::Z } else { Vec3::NEG_Z };
        let segments =
            crate::cracks::face_cracks(grid, look.cracks, look.seed, front, (i0, j0, i1, j1));
        for s in segments {
            let d = (s.b - s.a).normalize_or(Vec2::X);
            let side = Vec2::new(-d.y, d.x);
            let (gx, gy) = (
                grid.global.0 + ((s.a.x - grid.origin.x) / grid.cell.x) as i64,
                grid.global.1 + ((s.a.y - grid.origin.y) / grid.cell.y) as i64,
            );
            let color = scale(look.outside_color(s.a, gx, gy), 0.18);
            let at = |p: Vec2, w: f32, k: f32| {
                let q = p + side * (w * 0.5 * k);
                Vec3::new(q.x, q.y, z)
            };
            mb.quad_facing(
                [
                    at(s.a, s.width_a, -1.0),
                    at(s.b, s.width_b, -1.0),
                    at(s.b, s.width_b, 1.0),
                    at(s.a, s.width_a, 1.0),
                ],
                n,
                color,
            );
        }
    }

    // Exposed rebar.
    let radius = 0.008;
    for j in j0..j1 {
        for i in i0..i1 {
            let c = grid.idx(i, j);
            if grid.solid[c] || grid.rebar[c] == 0 {
                continue;
            }
            let (gx, gy) = (grid.global.0 + i as i64, grid.global.1 + j as i64);
            let x0 = grid.origin.x + i as f32 * grid.cell.x;
            let y0 = grid.origin.y + j as f32 * grid.cell.y;
            let wob = |a: i64, b: i64, s: u64| (hash3(a, b, s as i64, look.seed) - 0.5) * 0.012;
            if grid.rebar[c] & VBAR != 0 {
                let x = x0 + grid.cell.x * 0.5;
                let a = Vec3::new(x + wob(gx, gy, 21), y0, wob(gx, gy, 22));
                let b = Vec3::new(
                    x + wob(gx, gy + 1, 21),
                    y0 + grid.cell.y,
                    wob(gx, gy + 1, 22),
                );
                mb.rod(a, b, radius, look.rebar);
            }
            if grid.rebar[c] & HBAR != 0 {
                let y = y0 + grid.cell.y * 0.5;
                let a = Vec3::new(x0, y + wob(gx, gy, 23), 0.012 + wob(gx, gy, 24));
                let b = Vec3::new(
                    x0 + grid.cell.x,
                    y + wob(gx + 1, gy, 23),
                    0.012 + wob(gx + 1, gy, 24),
                );
                mb.rod(a, b, radius, look.rebar);
            }
        }
    }
    // Offset last, so texture coordinates stay in wall space (a chunk keeps its bricks).
    mb.translate(offset);
    mb
}

/// Solid cells merged into as few rectangles as possible `(i0, j0, i1, j1)` (exclusive),
/// for box colliders.
pub fn greedy_rects(grid: &CellGrid) -> Vec<(usize, usize, usize, usize)> {
    let (nx, ny) = (grid.nx, grid.ny);
    let mut used = vec![false; nx * ny];
    let mut out = Vec::new();
    for j in 0..ny {
        let mut i = 0;
        while i < nx {
            let idx = grid.idx(i, j);
            if !grid.solid[idx] || used[idx] {
                i += 1;
                continue;
            }
            let mut i_end = i + 1;
            while i_end < nx && grid.solid[grid.idx(i_end, j)] && !used[grid.idx(i_end, j)] {
                i_end += 1;
            }
            let mut j_end = j + 1;
            'grow: while j_end < ny {
                for k in i..i_end {
                    let idx = grid.idx(k, j_end);
                    if !grid.solid[idx] || used[idx] {
                        break 'grow;
                    }
                }
                j_end += 1;
            }
            for jj in j..j_end {
                for k in i..i_end {
                    used[grid.idx(k, jj)] = true;
                }
            }
            out.push((i, j, i_end, j_end));
            i = i_end;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material::props;

    fn state(wall: &DestructibleWall) -> WallState {
        WallState::new(wall, &DestructionMaterials::default())
    }

    #[test]
    fn soft_wall_has_studs_and_full_volume() {
        let wall = DestructibleWall::soft(3.0, 2.4);
        let s = state(&wall);
        assert_eq!((s.grid.nx, s.grid.ny), (50, 40));
        assert!(s.grid.kind.contains(&CellKind::Stud));
        assert!((s.integrity() - 1.0).abs() < 1e-5);
        assert!((s.grid.volume() - 3.0 * 2.4 * 0.12).abs() < 1e-3);
    }

    #[test]
    fn bullets_crater_then_punch_through_plaster() {
        let wall = DestructibleWall::soft(3.0, 2.4);
        let mut s = state(&wall);
        let p = props(DestructibleMaterial::Plaster);
        // Aim between studs.
        let at = Vec3::new(0.2, 1.2, 1.0);
        let hit = s.crater(&p, at, Vec3::NEG_Z, 330.0, 0.02, 1.0);
        assert!(hit.volume > 0.0);
        assert!(hit.exit.is_some(), "a rifle round goes through drywall");
        assert!(!hit.lost.is_empty());
    }

    #[test]
    fn bullets_only_chip_concrete() {
        let wall = DestructibleWall::concrete(3.0, 2.4, 0.3);
        let mut s = state(&wall);
        let p = props(DestructibleMaterial::Concrete);
        let hit = s.crater(&p, Vec3::new(0.0, 1.2, 1.0), Vec3::NEG_Z, 330.0, 0.02, 1.0);
        assert!(hit.volume > 0.0);
        assert!(hit.exit.is_none());
        assert!(hit.lost.is_empty());
        // The surface is dented on the front only.
        let c = s.grid.cell_at(Vec2::new(0.0, 1.2)).unwrap();
        assert!(s.grid.front[c] > 0.0 && s.grid.back[c] == 0.0);
    }

    #[test]
    fn breach_charge_blows_a_doorway_sized_hole_in_a_soft_wall() {
        let wall = DestructibleWall::soft(4.0, 3.0);
        let mut s = state(&wall);
        let p = props(DestructibleMaterial::Plaster);
        let hit = s.blast(&p, Vec3::new(0.0, 1.0, 0.1), 2.5e5, 2.5e6);
        let lost_area = hit.lost.len() as f32 * s.grid.cell.x * s.grid.cell.y;
        assert!(
            (0.4..4.0).contains(&lost_area),
            "hole of {lost_area} m² should be roughly a crouched doorway"
        );
        let islands = s.detach_unsupported();
        assert!(islands.is_empty(), "a hole in a framed wall drops nothing");
    }

    #[test]
    fn reinforced_wall_keeps_rebar_until_cut() {
        let wall = DestructibleWall::reinforced(3.0, 3.0);
        let mut s = state(&wall);
        let p = props(DestructibleMaterial::ReinforcedConcrete);
        let hit = s.blast(&p, Vec3::new(0.0, 1.5, 0.15), 4.0e6, 2.5e6);
        assert!(
            !hit.lost.is_empty(),
            "a big charge still breaks the concrete"
        );
        let rebar_left = hit.lost.iter().filter(|c| s.grid.rebar[c.idx] != 0).count();
        assert!(rebar_left > 0, "rebar survives at the edge of the hole");
        let cut = s.cut_rect(Vec2::new(-0.6, 0.5), Vec2::new(0.6, 2.5));
        assert!(!cut.lost.is_empty());
        let c = s.grid.cell_at(Vec2::new(0.0, 1.5)).unwrap();
        assert_eq!(s.grid.rebar[c], 0, "thermite cuts the rebar");
    }

    #[test]
    fn cutting_a_barricade_plank_at_both_ends_drops_it() {
        let wall = DestructibleWall::barricade(1.2, 1.0);
        let mut s = state(&wall);
        // Remove two columns across every plank near both sides.
        let left = s.cut_rect(Vec2::new(-0.5, 0.0), Vec2::new(-0.45, 1.0));
        let right = s.cut_rect(Vec2::new(0.45, 0.0), Vec2::new(0.5, 1.0));
        assert!(!left.lost.is_empty() && !right.lost.is_empty());
        let islands = s.detach_unsupported();
        assert!(
            islands.len() >= 3,
            "each plank falls on its own ({} islands)",
            islands.len()
        );
    }

    #[test]
    fn undercut_freestanding_wall_topples_its_top() {
        let wall = DestructibleWall::masonry(DestructibleMaterial::Stone, 3.0, 3.0, 0.6)
            .with_anchors(WallAnchors::GROUND);
        let mut s = state(&wall);
        let cut = s.cut_rect(Vec2::new(-1.6, 0.9), Vec2::new(1.6, 1.2));
        assert!(!cut.lost.is_empty());
        let islands = s.detach_unsupported();
        assert_eq!(islands.len(), 1);
        assert!(islands[0].len() > 100);
    }

    #[test]
    fn mesh_and_colliders_follow_damage() {
        let wall = DestructibleWall::reinforced(2.0, 2.0);
        let mut s = state(&wall);
        let look = WallLook::new(&wall, &DestructionMaterials::default());
        let all = (0, 0, s.grid.nx, s.grid.ny);
        let intact = mesh_cells(&s.grid, &look, all, Vec3::ZERO);
        assert_eq!(greedy_rects(&s.grid).len(), 1);
        let p = props(DestructibleMaterial::ReinforcedConcrete);
        s.blast(&p, Vec3::new(0.0, 1.0, 0.13), 3.0e6, 2.5e6);
        let damaged = mesh_cells(&s.grid, &look, all, Vec3::ZERO);
        assert!(
            damaged.indices.len() > intact.indices.len(),
            "edges and rebar add triangles"
        );
        assert!(greedy_rects(&s.grid).len() > 1);
        for v in &damaged.vertices {
            assert!(v.position.iter().all(|c| c.is_finite()));
            assert!((Vec3::from(v.normal).length() - 1.0).abs() < 1e-3);
        }
    }

    #[test]
    fn chunk_grid_keeps_shape_of_lost_cells() {
        let wall = DestructibleWall::concrete(2.0, 2.0, 0.2);
        let mut s = state(&wall);
        let cut = s.cut_rect(Vec2::new(-0.3, 0.5), Vec2::new(0.3, 1.0));
        let chunk = s.chunk_grid(&cut.lost);
        assert_eq!(chunk.solid_count(), cut.lost.len());
        assert!((chunk.volume() - cut.volume).abs() < 1e-4);
        let mut look = WallLook::new(&wall, &DestructionMaterials::default());
        look.straight_border = false;
        let m = mesh_cells(&chunk, &look, (0, 0, chunk.nx, chunk.ny), Vec3::ZERO);
        assert!(!m.is_empty());
    }
}
