//! Foliage's half of [`engine_world::GroundCover`]: what grows on the ground around the focus,
//! written from the grass fields foliage and others draw (their kinds, density, dryness and
//! wear maps), one meter at a time. Terrain shades meadow ground past the last blades from it,
//! footsteps swish through tall grass, movement slows in it, AI weighs who it hides, and the
//! weather lays snow on it, none of them knowing how grass is drawn.
//!
//! The grid follows the primary focus in steps of [`STEP_M`]: [`Grid2::recenter`] keeps what
//! it already knew and only the strips that came into it are painted. A field that is added,
//! changed or removed repaints its own rectangle. Painting a strip of 32 x 320 cells costs
//! well under a millisecond.

use engine_core::math::{DVec2, DVec3, IVec2};
use engine_core::prelude::*;
use engine_render::{GrassField, GrassStyle, GrassWear};
use engine_world::{CoverCell, CoverKind, FocusPoints, GroundCover};

use crate::grass::FloorLayer;
use crate::meadow::GrassKind;

/// Width of the grid, meters: past grass's 150 m reach, so terrain can shade the meadow where
/// the blades end.
pub const COVER_SIZE_M: f32 = 320.0;
/// Cell size, meters.
pub const COVER_CELL_M: f32 = 1.0;
/// How far the focus moves before the grid follows, meters.
pub const STEP_M: f64 = 32.0;

/// What grows where a style's blades stand, from how they look: for fields whose kinds
/// foliage didn't paint itself (a game's own meadow).
pub fn cover_kind(style: &GrassStyle, dryness: f32) -> CoverKind {
    let lum = |c: [f32; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    if style.lying > 0.5 {
        // Fallen leaves: nothing growing.
        CoverKind::Bare
    } else if style.leaf > 0.5 {
        if style.height >= 0.5 {
            CoverKind::Bracken
        } else {
            CoverKind::Forbs
        }
    } else if style.height <= 0.16 {
        CoverKind::Turf
    } else if style.bend <= 0.2 && style.clump_fan >= 0.5 && style.height >= 0.35 {
        // Stiff, upright, fanned tussocks: sedge and rush.
        CoverKind::Sedge
    } else if style.height < 0.4 && style.bend <= 0.2 && lum(style.lush_color) < 0.06 {
        CoverKind::Heather
    } else if dryness >= 0.6 {
        CoverKind::DryGrass
    } else if style.height >= 0.6 {
        CoverKind::TallGrass
    } else {
        CoverKind::Meadow
    }
}

/// What a meadow kind ([`crate::meadow::palette`]) is as ground cover.
pub fn meadow_cover(kind: GrassKind) -> CoverKind {
    use GrassKind::*;
    match kind {
        Meadow | Buttercups | Daisies | Harebells => CoverKind::Meadow,
        Turf | RedClover => CoverKind::Turf,
        Tall | Fireweed => CoverKind::TallGrass,
        Dry | Yarrow => CoverKind::DryGrass,
        Sedge => CoverKind::Sedge,
        Clover => CoverKind::Forbs,
        Bracken => CoverKind::Bracken,
        Heather => CoverKind::Heather,
    }
}

fn meadow_kind(i: u8) -> Option<GrassKind> {
    use GrassKind::*;
    const ALL: [GrassKind; crate::meadow::KINDS] = [
        Meadow, Turf, Tall, Dry, Sedge, Clover, Bracken, Heather, Buttercups, Daisies, RedClover,
        Yarrow, Harebells, Fireweed,
    ];
    ALL.get(i as usize).copied()
}

/// One field as the painter reads it: its maps and where it lies.
struct FieldView<'a> {
    field: &'a GrassField,
    wear: Option<&'a GrassWear>,
    /// World to field-local.
    to_local: engine_core::math::DAffine3,
    /// World X/Z bounds.
    min: DVec2,
    max: DVec2,
    /// Kinds painted by foliage's meadow palette.
    meadow: bool,
}

/// What one sample of the ground found.
#[derive(Clone, Copy)]
struct Found {
    kind: CoverKind,
    height: f32,
    dryness: f32,
    density: f32,
    flowers: f32,
}

impl<'a> FieldView<'a> {
    fn new(
        field: &'a GrassField,
        at: &GlobalTransform,
        wear: Option<&'a GrassWear>,
        layer: Option<FloorLayer>,
    ) -> Self {
        let half = DVec3::new(field.size[0] as f64, 0.0, field.size[1] as f64) * 0.5;
        let corners = [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)].map(|(x, z)| {
            at.0.transform_point3(DVec3::new(half.x * x, 0.0, half.z * z))
        });
        let (mut min, mut max) = (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY));
        for c in corners {
            min = min.min(DVec2::new(c.x, c.z));
            max = max.max(DVec2::new(c.x, c.z));
        }
        Self {
            field,
            wear,
            to_local: at.0.inverse(),
            min,
            max,
            meadow: layer == Some(FloorLayer::Grass)
                && field.styles.len() + 1 == crate::meadow::KINDS,
        }
    }

    /// The map at grid point (x, z), clamped; bilinear for `values` (empty: `empty`).
    fn bilinear(&self, values: &[f32], u: f32, v: f32, empty: f32) -> f32 {
        let [rx, rz] = self.field.resolution;
        if values.len() != (rx * rz) as usize || rx < 2 || rz < 2 {
            return empty;
        }
        let (x, z) = (u.clamp(0.0, (rx - 1) as f32), v.clamp(0.0, (rz - 1) as f32));
        let (x0, z0) = (x.floor() as u32, z.floor() as u32);
        let (x1, z1) = ((x0 + 1).min(rx - 1), (z0 + 1).min(rz - 1));
        let (fx, fz) = (x - x0 as f32, z - z0 as f32);
        let at = |i: u32, j: u32| values[(j * rx + i) as usize];
        let a = at(x0, z0) + (at(x1, z0) - at(x0, z0)) * fx;
        let b = at(x0, z1) + (at(x1, z1) - at(x0, z1)) * fx;
        a + (b - a) * fz
    }

    fn sample(&self, p: DVec3) -> Option<Found> {
        if p.x < self.min.x || p.x > self.max.x || p.z < self.min.y || p.z > self.max.y {
            return None;
        }
        let l = self.to_local.transform_point3(p);
        let [sx, sz] = self.field.size;
        let (lx, lz) = (l.x as f32 + sx * 0.5, l.z as f32 + sz * 0.5);
        if !(0.0..=sx).contains(&lx) || !(0.0..=sz).contains(&lz) {
            return None;
        }
        let [rx, rz] = self.field.resolution.map(|r| r.max(2));
        let (u, v) = (lx / sx * (rx - 1) as f32, lz / sz * (rz - 1) as f32);
        let style_d = self.field.style.dryness;
        let mut density = self
            .bilinear(&self.field.density, u, v, 1.0)
            .clamp(0.0, 1.0);
        let mut dryness = self
            .bilinear(&self.field.dryness, u, v, style_d)
            .clamp(0.0, 1.0);
        let k = {
            let (i, j) = (u.round() as u32, v.round() as u32);
            self.field
                .kinds
                .get((j.min(rz - 1) * rx + i.min(rx - 1)) as usize)
                .copied()
                .unwrap_or(0)
        };
        let style = self
            .field
            .palette()
            .nth(k as usize)
            .unwrap_or(&self.field.style);
        let mut kind = match self.meadow.then(|| meadow_kind(k)).flatten() {
            Some(m) => {
                let c = meadow_cover(m);
                // A meadow parched to straw reads as dry grass whatever grows there.
                if dryness >= 0.75 && matches!(c, CoverKind::Meadow | CoverKind::TallGrass) {
                    CoverKind::DryGrass
                } else {
                    c
                }
            }
            None => cover_kind(style, dryness),
        };
        let mut height = style.height;
        // Worn ground: trodden short, then bare.
        if let Some(w) = self.wear {
            let [wx, wz] = w.resolution;
            if wx >= 2 && wz >= 2 && w.cell > 0.0 && w.wear.len() == (wx * wz) as usize {
                let (wu, wv) = ((lx - w.origin[0]) / w.cell, (lz - w.origin[1]) / w.cell);
                if (0.0..=(wx - 1) as f32).contains(&wu) && (0.0..=(wz - 1) as f32).contains(&wv) {
                    let view = FieldView {
                        field: &GrassField {
                            resolution: w.resolution,
                            ..Default::default()
                        },
                        wear: None,
                        to_local: self.to_local,
                        min: self.min,
                        max: self.max,
                        meadow: false,
                    };
                    let worn = view.bilinear(&w.wear, wu, wv, 0.0).clamp(0.0, 1.0);
                    height *= 1.0 - 0.8 * worn;
                    density *= 1.0 - worn * worn;
                    dryness += (1.0 - dryness) * 0.5 * worn;
                    if worn > 0.85 {
                        kind = CoverKind::Bare;
                    } else if worn > 0.4 && kind != CoverKind::Bare {
                        kind = CoverKind::Turf;
                    }
                }
            }
        }
        if kind == CoverKind::Bare {
            density = 0.0;
            height = 0.0;
        }
        Some(Found {
            kind,
            height: height * (0.4 + 0.6 * density.sqrt()),
            dryness,
            density,
            flowers: if style.bloom_size > 0.0 {
                style.bloom.clamp(0.0, 1.0)
            } else {
                0.0
            },
        })
    }
}

/// The cover of one cell from four samples a quarter cell in from its corners: the kind most
/// of the ground has, the next most mixed in, and the averages.
fn paint_cell(fields: &[FieldView<'_>], center: DVec2, y: f64, cell: f64) -> CoverCell {
    let mut weight = [0.0f32; CoverKind::ALL.len()];
    let (mut height, mut dryness, mut density, mut flowers, mut n) = (0.0, 0.0, 0.0, 0.0, 0.0f32);
    for (dx, dz) in [(-0.25, -0.25), (0.25, -0.25), (-0.25, 0.25), (0.25, 0.25)] {
        let p = DVec3::new(center.x + dx * cell, y, center.y + dz * cell);
        // Overlapping fields (a forest's floor layers over its meadow): the densest growth.
        let best = fields.iter().filter_map(|f| f.sample(p)).max_by(|a, b| {
            (a.density * a.height.max(0.01)).total_cmp(&(b.density * b.height.max(0.01)))
        });
        n += 1.0;
        let Some(s) = best else {
            continue;
        };
        weight[s.kind as usize] += s.density.max(0.05);
        height += s.height;
        dryness += s.dryness;
        density += s.density;
        flowers += s.flowers;
    }
    // The two kinds most of the ground has (no allocation: this runs for every cell).
    let (mut first, mut second) = (usize::MAX, usize::MAX);
    for (i, &w) in weight.iter().enumerate() {
        if w <= 0.0 {
            continue;
        }
        if first == usize::MAX || w > weight[first] {
            second = first;
            first = i;
        } else if second == usize::MAX || w > weight[second] {
            second = i;
        }
    }
    if first == usize::MAX {
        return CoverCell::default();
    }
    if second == usize::MAX {
        second = first;
    }
    let share = if second == first {
        0.0
    } else {
        (weight[second] / (weight[first] + weight[second])).min(0.5)
    };
    CoverCell {
        kind: CoverKind::ALL[first],
        second: CoverKind::ALL[second],
        second_share: share,
        height_m: height / n,
        dryness: dryness / n,
        density: density / n,
        flowers: flowers / n,
    }
}

/// Paints the cells `[min, max)` of the grid.
fn paint_rect(cover: &mut GroundCover, fields: &[FieldView<'_>], y: f64, min: IVec2, max: IVec2) {
    let cell = cover.grid.cell as f64;
    for j in min.y.max(0)..max.y.min(cover.grid.height as i32) {
        for i in min.x.max(0)..max.x.min(cover.grid.width as i32) {
            let c = IVec2::new(i, j);
            let center = cover.grid.cell_center(c);
            let v = paint_cell(fields, center, y, cell);
            cover.grid.set(c, v);
        }
    }
}

/// The grid's cells covering world X/Z `[min, max]`.
fn cells_of(cover: &GroundCover, min: DVec2, max: DVec2) -> (IVec2, IVec2) {
    let g = &cover.grid;
    let to = |p: DVec2| ((p - g.origin) / g.cell as f64).floor();
    let (a, b) = (to(min), to(max) + DVec2::ONE);
    let clamp = |v: DVec2| {
        IVec2::new(
            v.x.clamp(0.0, g.width as f64) as i32,
            v.y.clamp(0.0, g.height as f64) as i32,
        )
    };
    (clamp(a), clamp(b))
}

/// Cells painted per frame at most: a 320 m grid fills in about 25 frames and a 32 m step's
/// strips in three to five, at about half a millisecond a frame.
pub const CELLS_PER_FRAME: usize = 4096;

/// What the painter remembers between frames.
#[derive(Resource)]
pub struct CoverPainted {
    /// Field rectangles painted last time (world X/Z), to repaint where a removed or moved
    /// field was.
    fields: Vec<(Entity, DVec2, DVec2)>,
    /// Rectangles still to paint (world X/Z, so they stay put when the grid moves).
    pending: std::collections::VecDeque<(DVec2, DVec2)>,
    /// Cells painted per frame at most.
    pub budget: usize,
}

impl Default for CoverPainted {
    fn default() -> Self {
        Self {
            fields: Vec::new(),
            pending: Default::default(),
            budget: CELLS_PER_FRAME,
        }
    }
}

impl CoverPainted {
    /// Nothing is waiting to be painted.
    pub fn is_settled(&self) -> bool {
        self.pending.is_empty()
    }
}

type CoverFields<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        Ref<'static, GrassField>,
        Ref<'static, GlobalTransform>,
        Option<Ref<'static, GrassWear>>,
        Option<&'static FloorLayer>,
    ),
>;

/// Keeps [`GroundCover`] centered on the primary focus and painted from every grass field.
pub fn paint_ground_cover(
    focus: Res<FocusPoints>,
    mut cover: ResMut<GroundCover>,
    mut painted: ResMut<CoverPainted>,
    fields: CoverFields,
) {
    let at = focus.primary();
    let here = DVec2::new(at.x, at.z);
    let views: Vec<FieldView<'_>> = fields
        .iter()
        .map(|(_, f, t, w, l)| {
            FieldView::new(
                f.into_inner(),
                t.into_inner(),
                w.map(|w| w.into_inner()),
                l.copied(),
            )
        })
        .collect();
    let size = COVER_SIZE_M;
    let mut dirty: Vec<(IVec2, IVec2)> = Vec::new();
    let g = &cover.grid;
    let center = g.origin + DVec2::new(g.width as f64, g.height as f64) * g.cell as f64 * 0.5;
    let mut whole = false;
    if g.width == 0 || g.cell != COVER_CELL_M {
        *cover = GroundCover::around(here, size, COVER_CELL_M);
        painted.pending.clear();
        whole = true;
        dirty.push((
            IVec2::ZERO,
            IVec2::new(cover.grid.width as i32, cover.grid.height as i32),
        ));
    } else if (here - center).abs().max_element() > STEP_M {
        let old = cover.grid.origin;
        cover.grid.recenter(here);
        let shift = ((cover.grid.origin - old) / cover.grid.cell as f64).round();
        let (w, h) = (cover.grid.width as i32, cover.grid.height as i32);
        let (dx, dy) = (shift.x as i32, shift.y as i32);
        // The strips that came in: columns on the side moved toward, rows likewise.
        if dx > 0 {
            dirty.push((IVec2::new((w - dx).max(0), 0), IVec2::new(w, h)));
        } else if dx < 0 {
            dirty.push((IVec2::ZERO, IVec2::new((-dx).min(w), h)));
        }
        if dy > 0 {
            dirty.push((IVec2::new(0, (h - dy).max(0)), IVec2::new(w, h)));
        } else if dy < 0 {
            dirty.push((IVec2::ZERO, IVec2::new(w, (-dy).min(h))));
        }
    }
    // Fields that came, changed or moved repaint where they are now and where they were.
    let mut now = Vec::with_capacity(views.len());
    for ((e, f, t, w, _), v) in fields.iter().zip(&views) {
        now.push((e, v.min, v.max));
        let before = painted.fields.iter().find(|(pe, _, _)| *pe == e);
        let changed = f.is_changed() || t.is_changed() || w.is_some_and(|w| w.is_changed());
        if (changed || before.is_none()) && !whole {
            dirty.push(cells_of(&cover, v.min, v.max));
            if let Some(&(_, a, b)) = before.filter(|(_, a, b)| (*a, *b) != (v.min, v.max)) {
                dirty.push(cells_of(&cover, a, b));
            }
        }
    }
    for &(e, a, b) in &painted.fields {
        if !whole && !now.iter().any(|(ne, _, _)| *ne == e) {
            dirty.push(cells_of(&cover, a, b));
        }
    }
    painted.fields = now;
    let (origin, cell) = (cover.grid.origin, cover.grid.cell as f64);
    for (min, max) in dirty {
        if min.x < max.x && min.y < max.y {
            let a = origin + min.as_dvec2() * cell;
            let b = origin + max.as_dvec2() * cell;
            painted.pending.push_back((a, b));
        }
    }
    // Paint what's waiting, nearest the focus first being the strips just come in, up to the
    // budget; a rectangle too big for what's left is painted a few rows at a time.
    let mut left = painted.budget.max(1);
    while left > 0 {
        let Some((a, b)) = painted.pending.pop_front() else {
            break;
        };
        // Cells of the rectangle, a little inside its edges so neighbours aren't caught.
        let inset = DVec2::splat(cell * 0.25);
        let (min, max) = cells_of(&cover, a + inset, b - inset);
        let (w, h) = (
            (max.x - min.x).max(0) as usize,
            (max.y - min.y).max(0) as usize,
        );
        if w == 0 || h == 0 {
            continue;
        }
        let rows = (left / w).max(1).min(h);
        let cut = IVec2::new(max.x, min.y + rows as i32);
        paint_rect(&mut cover, &views, at.y, min, cut);
        left = left.saturating_sub(rows * w);
        if rows < h {
            let origin = cover.grid.origin;
            let rest = origin + DVec2::new(min.x as f64, cut.y as f64) * cell;
            painted.pending.push_front((rest, b));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_render::GrassStyle;

    #[test]
    fn every_meadow_kind_reads_as_its_own_cover_from_its_look_alone() {
        // A game's meadow built from `meadow::palette` but not painted by foliage still gets
        // the right cover from how each kind looks.
        let meadow = GrassStyle::meadow();
        let palette = crate::meadow::palette(&meadow, 175.0);
        let styles: Vec<GrassStyle> = std::iter::once(meadow).chain(palette).collect();
        for (i, style) in styles.iter().enumerate() {
            let kind = meadow_kind(i as u8).unwrap();
            let want = meadow_cover(kind);
            let dry = if want == CoverKind::DryGrass {
                0.65
            } else {
                0.1
            };
            assert_eq!(cover_kind(style, dry), want, "{kind:?}");
        }
        assert_eq!(cover_kind(&GrassStyle::leaf_litter(), 0.3), CoverKind::Bare);
        assert_eq!(
            cover_kind(&GrassStyle::forest_herbs(), 0.1),
            CoverKind::Forbs
        );
    }

    fn app() -> App {
        let mut app = App::new();
        app.world.init_resource::<FocusPoints>();
        app.world.init_resource::<GroundCover>();
        app.world.init_resource::<CoverPainted>();
        app.add_systems(Update, paint_ground_cover);
        app
    }

    /// Runs frames until the painter has caught up (it paints a budget of cells a frame).
    fn settle(app: &mut App) {
        app.update();
        for _ in 0..64 {
            if app.world.resource::<CoverPainted>().is_settled() {
                return;
            }
            app.update();
        }
        panic!("the ground cover never settled");
    }

    fn meadow_field(at: DVec3, size: f32) -> (Transform, GlobalTransform, GrassField) {
        let style = GrassStyle::meadow();
        let mut field = GrassField::flat(size, size)
            .sample([size as u32 + 1, size as u32 + 1], |x, _| {
                // Dense at -x, sparse toward +x.
                (0.0, (0.5 - x / size).clamp(0.1, 1.0), 0.1)
            })
            .with_style(style);
        let n = field.density.len();
        // Kind 2 (tall grass) on the +z half.
        let [rx, _] = field.resolution;
        field.kinds = (0..n)
            .map(|i| if i as u32 / rx > rx / 2 { 2 } else { 0 })
            .collect();
        field.styles = crate::meadow::palette(&style, 175.0);
        let t = Transform::from_translation(at);
        (t, GlobalTransform::from(t), field)
    }

    #[test]
    fn the_cover_follows_the_fields_and_the_focus() {
        let mut app = app();
        let (t, g, f) = meadow_field(DVec3::new(10.0, 0.0, 0.0), 60.0);
        let e = app.world.spawn((t, g, f, FloorLayer::Grass)).id();
        settle(&mut app);
        let cover = app.world.resource::<GroundCover>();
        let low = cover.cell(DVec3::new(0.0, 0.0, -10.0)).unwrap();
        let high = cover.cell(DVec3::new(0.0, 0.0, 15.0)).unwrap();
        assert_eq!(low.kind, CoverKind::Meadow);
        assert_eq!(high.kind, CoverKind::TallGrass);
        assert!(high.height_m > low.height_m + 0.2, "{high:?} {low:?}");
        // Denser toward the field's -x side.
        let west = cover.cell(DVec3::new(-15.0, 0.0, -10.0)).unwrap();
        let east = cover.cell(DVec3::new(35.0, 0.0, -10.0)).unwrap();
        assert!(west.density > east.density + 0.3, "{west:?} {east:?}");
        // Outside the field nothing grows; outside the grid nothing is known.
        assert_eq!(
            cover.cell(DVec3::new(-60.0, 0.0, 0.0)).unwrap().kind,
            CoverKind::Bare
        );
        assert_eq!(cover.cell(DVec3::new(400.0, 0.0, 0.0)), None);

        // The focus walks 100 m east: the grid follows and still knows the field.
        app.world.resource_mut::<FocusPoints>().points = vec![DVec3::new(100.0, 0.0, 0.0)];
        settle(&mut app);
        let cover = app.world.resource::<GroundCover>();
        assert_eq!(
            cover.cell(DVec3::new(250.0, 0.0, 0.0)).unwrap().kind,
            CoverKind::Bare
        );
        assert_eq!(cover.cell(DVec3::new(-100.0, 0.0, 0.0)), None);
        assert_eq!(
            cover.cell(DVec3::new(0.0, 0.0, 15.0)).unwrap().kind,
            CoverKind::TallGrass
        );

        // The field goes: its ground is bare again.
        app.world.despawn(e);
        settle(&mut app);
        let cover = app.world.resource::<GroundCover>();
        assert_eq!(
            cover.cell(DVec3::new(0.0, 0.0, 15.0)).unwrap().kind,
            CoverKind::Bare
        );
    }

    #[test]
    fn a_worn_path_is_turf_then_bare() {
        let mut app = app();
        let (t, g, f) = meadow_field(DVec3::ZERO, 40.0);
        // Worn along x = 20 (local, from the -x corner): bare within a meter, trodden beside.
        let wear = GrassWear {
            origin: [16.0, 0.0],
            cell: 0.5,
            resolution: [17, 81],
            wear: (0..17 * 81)
                .map(|i| {
                    let x = 16.0 + (i % 17) as f32 * 0.5;
                    ((3.0 - (x - 20.0).abs()) / 1.5).clamp(0.0, 1.0)
                })
                .collect(),
        };
        app.world.spawn((t, g, f, wear, FloorLayer::Grass));
        settle(&mut app);
        let cover = app.world.resource::<GroundCover>();
        let mid = cover.cell(DVec3::new(0.2, 0.0, -10.0)).unwrap();
        let side = cover.cell(DVec3::new(1.5, 0.0, -10.0)).unwrap();
        let off = cover.cell(DVec3::new(8.0, 0.0, -10.0)).unwrap();
        assert_eq!(mid.kind, CoverKind::Bare, "{mid:?}");
        assert_eq!(side.kind, CoverKind::Turf, "{side:?}");
        assert_eq!(off.kind, CoverKind::Meadow, "{off:?}");
        assert!(off.height_m > side.height_m);
    }
}

#[cfg(test)]
mod bench {
    use super::*;

    /// `cargo test --release -p engine_foliage --lib cover_cost -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn cover_cost() {
        let mut app = App::new();
        app.world.init_resource::<FocusPoints>();
        app.world.init_resource::<GroundCover>();
        app.world.insert_resource(CoverPainted {
            budget: usize::MAX,
            ..Default::default()
        });
        app.add_systems(Update, paint_ground_cover);
        let style = GrassStyle::meadow();
        for k in 0..3 {
            let size = 300.0;
            let mut field = GrassField::flat(size, size)
                .sample([301, 301], |x, z| (0.0, 0.5 + 0.004 * (x - z), 0.2))
                .with_style(style);
            field.kinds = (0..field.density.len()).map(|i| (i % 14) as u8).collect();
            field.styles = crate::meadow::palette(&style, 175.0);
            let t = Transform::from_xyz(k as f64 * 40.0 - 40.0, 0.0, 0.0);
            app.world.spawn((
                t,
                GlobalTransform::from(t),
                field,
                crate::grass::FloorLayer::Grass,
            ));
        }
        let started = std::time::Instant::now();
        app.update();
        let full = started.elapsed();
        app.world.resource_mut::<FocusPoints>().points = vec![DVec3::new(40.0, 0.0, 0.0)];
        let started = std::time::Instant::now();
        app.update();
        let step = started.elapsed();
        let cells = (COVER_SIZE_M / COVER_CELL_M).powi(2);
        println!(
            "ground cover over 3 fields: {cells} cells in {full:?} ({:.0} ns a cell); a 40 m step {step:?}",
            full.as_nanos() as f32 / cells
        );
    }
}
