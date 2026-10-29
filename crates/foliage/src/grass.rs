//! Meadow grass on the GPU: every ecology region where grass grows gets one
//! `engine_render::GrassField` over its rectangle, so the ground layer is millions of blades
//! that sway, part around characters (`GrassBender`) and thin out with distance, instead of
//! one mesh per tuft.
//!
//! The ecology stays the source of truth. Each living grass plant in the region spreads its
//! cover over the field's density map (a healthy, grown tuft covers about a square meter),
//! so grass is thick in the clear-cut and the burn scar, thin under the canopy and absent
//! where nothing lets it grow: keep-clear circles (the trail, the pond), logs, rocks, water.
//! Dryness follows the soil's water and the grass's own health, so a drought or a fire
//! browns it, and the season ([`crate::season::grass_dryness`]) turns it to straw in winter.
//! The maps are rebuilt every few seconds; heights come from `Ground` once and again after
//! `TerrainChanged`. Grass plants under a field draw nothing themselves.
//!
//! A forest's floor gets two more fields over the same rectangle ([`FloorLayer`]), as games
//! layer a floor's decoration from the maps its ground is painted with (Frostbite, Horizon Zero
//! Dawn; engine-design/foliage-research.md, 6): low shade herbs in rosettes where the crowns
//! close over and grass gives way, in colonies, thicker on moist soil; and fallen leaves lying
//! under broadleaf crowns. Neither grows on the trail, under logs and rocks, or in a trunk.
//! Both stand on the forest floor's own surface ([`GroundSurface`], a few centimeters of humus
//! and litter above `Ground`, which would bury them), and follow its painted layers: leaves
//! where litter and humus are painted, no herbs on mud and rock.

use engine_core::math::{noise::fbm2, smoothstep};
use engine_core::prelude::*;
use engine_render::{GrassField, GrassStyle};
use engine_world::Ground;

use std::sync::{Arc, Mutex};

use engine_core::tasks::AsyncComputeTaskPool;

use crate::ecology::{InRegion, RegionState};
use crate::felling::{LOG_BARE_YEARS, LogPlaced};
use crate::plant::{EcologyRegion, EcologySettings, FelledLog, Plant, PlantStage};
use crate::scenery::{GroundLayer, GroundSurface, GroundVertex, Rock};
use crate::season::{FoliageSeason, grass_dryness};
use crate::sim::Soil;
use crate::species::{PlantForm, PlantLibrary};

/// Spacing of the field's map grid, meters.
pub const GRASS_CELL_M: f32 = 1.0;
/// Most grid points along one side of a field's maps.
const MAX_RESOLUTION: u32 = 512;
/// Seconds (real time) between map rebuilds.
const REBUILD_SECS: f32 = 3.0;

/// On a region: the entity drawing its GPU grass.
#[derive(Component, Debug, Clone, Copy)]
pub struct RegionGrass(pub Entity);

/// On a grass field entity: the region it belongs to.
#[derive(Component, Debug, Clone, Copy)]
pub struct GrassOf(pub Entity);

/// What a region's GPU field draws: its meadow grass, or a layer of a forest's floor.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FloorLayer {
    Grass,
    /// Low shade herbs in rosettes ([`GrassStyle::forest_herbs`]).
    Herbs,
    /// Fallen leaves ([`GrassStyle::leaf_litter`]).
    Litter,
}

/// On a region: the entities drawing its forest floor's herbs and fallen leaves.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct RegionFloor {
    pub herbs: Option<Entity>,
    pub litter: Option<Entity>,
}

/// The fields a region draws and how each looks: grass where it has a grass species, and in a
/// forest (any tree species) herbs, and fallen leaves where some trees are broadleaved.
pub fn floor_styles(
    library: &PlantLibrary,
    region: &EcologyRegion,
) -> Vec<(FloorLayer, GrassStyle)> {
    let Some(grass) = region_style(library, region) else {
        return Vec::new();
    };
    let forms: Vec<PlantForm> = region
        .species
        .iter()
        .filter_map(|n| library.id(n))
        .map(|id| library.get(id).form)
        .collect();
    let mut out = vec![(FloorLayer::Grass, grass)];
    if forms
        .iter()
        .any(|f| matches!(f, PlantForm::Broadleaf | PlantForm::Conifer))
    {
        out.push((FloorLayer::Herbs, GrassStyle::forest_herbs()));
    }
    if forms.contains(&PlantForm::Broadleaf) {
        out.push((FloorLayer::Litter, GrassStyle::leaf_litter()));
    }
    out
}

/// A tree or shrub as the floor sees it: positions in meters from the region's center.
#[derive(Debug, Clone, Copy)]
pub struct Crown {
    pub pos: Vec2,
    /// Crown radius, meters.
    pub radius: f32,
    /// How much it shades and sheds (alive and grown most), 0..1.
    pub leafy: f32,
    /// Sheds broad leaves (rather than needles, which the painted ground shows).
    pub broadleaf: bool,
}

/// Herbs' and fallen leaves' density and dryness maps (in that order) over the same grid as
/// [`grass_maps`], from the crowns overhead, the soil and the grass's own `grass` density.
#[allow(clippy::too_many_arguments)]
pub fn floor_maps(
    region: &EcologyRegion,
    soil: &Soil,
    center: DVec3,
    resolution: [u32; 2],
    grass: &[f32],
    crowns: &[Crown],
    covers: &[Cover],
    surfaces: &[CoverGrid],
    season_dry: f32,
) -> [(Vec<f32>, Vec<f32>); 2] {
    let [rx, rz] = resolution;
    let size = Vec2::new(region.half_extents[0] * 2.0, region.half_extents[1] * 2.0);
    let step = Vec2::new(size.x / (rx - 1) as f32, size.y / (rz - 1) as f32);
    let corner = -size * 0.5;
    let n = (rx * rz) as usize;
    // How closed the canopy is over each point, and how much of it is broadleaved.
    let mut canopy = vec![0.0f32; n];
    let mut broad = vec![0.0f32; n];
    for c in crowns {
        let reach = c.radius.max(step.x.max(step.y));
        let ci = ((c.pos.x - corner.x) / step.x).round() as i64;
        let cj = ((c.pos.y - corner.y) / step.y).round() as i64;
        let (si, sj) = (
            (reach / step.x).ceil() as i64,
            (reach / step.y).ceil() as i64,
        );
        for j in (cj - sj).max(0)..=(cj + sj).min(rz as i64 - 1) {
            for i in (ci - si).max(0)..=(ci + si).min(rx as i64 - 1) {
                let q = corner + Vec2::new(i as f32 * step.x, j as f32 * step.y);
                let d = q.distance(c.pos) / reach;
                if d >= 1.0 {
                    continue;
                }
                let f = (1.0 - d * d) * c.leafy;
                let k = (j as u32 * rx + i as u32) as usize;
                canopy[k] += f;
                if c.broadleaf {
                    broad[k] += f;
                }
            }
        }
    }
    let bare = bare_mask(region, resolution, covers);
    let mut herbs = (Vec::with_capacity(n), Vec::with_capacity(n));
    let mut litter = (Vec::with_capacity(n), Vec::with_capacity(n));
    for j in 0..rz {
        for i in 0..rx {
            let k = (j * rx + i) as usize;
            let q = corner + Vec2::new(i as f32 * step.x, j as f32 * step.y);
            let world = center + DVec3::new(q.x as f64, 0.0, q.y as f64);
            let inside = soil.contains(world);
            let blocked = !inside || soil.blocked[soil.index(world)];
            let water = if inside { soil.sample(world)[0] } else { 0.5 };
            let (x, z) = (q.x as f64, q.y as f64);
            let open = 1.0 - 0.85 * grass.get(k).copied().unwrap_or(0.0);
            // What the floor is painted with here (none outside the covers: no say).
            let painted = surfaces.iter().find_map(|c| {
                c.at(world).map(|(_, w)| {
                    let leafy = w(GroundLayer::Litter) + 0.6 * w(GroundLayer::Humus);
                    let hostile = w(GroundLayer::Mud) + w(GroundLayer::Rock) + w(GroundLayer::Ash);
                    (leafy, hostile, w(GroundLayer::Needles))
                })
            });
            let (leafy, hostile, needles) = painted.unwrap_or((1.0, 0.0, 0.0));
            // Herbs spread in colonies a few meters across, under a closing canopy, thicker
            // where the soil stays moist.
            let colony = fbm2(x * 0.22, z * 0.22, region.seed ^ 0x4e8b, 3, 2.0, 0.5);
            let herb = if blocked || bare[k] {
                0.0
            } else {
                let d = smoothstep(0.2, 0.9, canopy[k])
                    * smoothstep(-0.2, 0.35, colony)
                    * (0.4 + 0.6 * smoothstep(0.15, 0.45, water))
                    * open
                    * (1.0 - hostile).max(0.0)
                    * (1.0 - 0.5 * needles);
                if d < 0.08 { 0.0 } else { d.min(1.0) }
            };
            let thirst = ((0.4 - water) / 0.3).clamp(0.0, 1.0);
            herbs.0.push(herb);
            herbs
                .1
                .push((0.05 + 0.6 * thirst + 0.5 * season_dry).min(1.0));
            // Fallen leaves lie thickest under broadleaf crowns, drifted into patches, and
            // fewer where grass or herbs cover the ground.
            let drift = fbm2(x * 0.3, z * 0.3, region.seed ^ 0x1177, 3, 2.0, 0.5);
            let leaves = if blocked || bare[k] {
                0.0
            } else {
                let d = smoothstep(0.1, 0.7, broad[k])
                    * (0.55 + 0.45 * smoothstep(-0.4, 0.4, drift))
                    * open
                    * (1.0 - 0.5 * herb)
                    * smoothstep(0.15, 0.6, leafy);
                if d < 0.06 { 0.0 } else { d.min(1.0) }
            };
            // Dryness picks the leaf's color, fresh brown to pale ochre, in patches.
            let hue = fbm2(x * 0.9, z * 0.9, region.seed ^ 0x2c3, 2, 2.0, 0.5);
            litter.0.push(leaves);
            litter
                .1
                .push((0.25 + 0.5 * smoothstep(-0.35, 0.45, hue) + 0.2 * season_dry).min(1.0));
        }
    }
    [herbs, litter]
}

/// A ground cover's surface as the floor fields see it: its grid of heights and painted layers.
#[derive(Clone)]
pub struct CoverGrid {
    /// The cover's center (its vertices are local to it).
    center: DVec3,
    half: [f32; 2],
    cell: f32,
    nx: usize,
    nz: usize,
    vertices: Arc<Vec<GroundVertex>>,
    slots: [GroundLayer; 8],
}

impl CoverGrid {
    pub fn new(surface: &GroundSurface, center: DVec3) -> Option<Self> {
        let cover = &surface.built_from;
        let cell = cover.cell_m.max(0.2);
        let nx = (cover.half_extents[0] * 2.0 / cell).ceil() as usize + 1;
        let nz = (cover.half_extents[1] * 2.0 / cell).ceil() as usize + 1;
        (nx >= 2 && nz >= 2 && surface.vertices.len() == nx * nz).then(|| Self {
            center,
            half: cover.half_extents,
            cell,
            nx,
            nz,
            vertices: surface.vertices.clone(),
            slots: surface.slots,
        })
    }

    /// The surface's height (world y) at `p`, and how much of each layer is painted there.
    pub fn at(&self, p: DVec3) -> Option<(f64, impl Fn(GroundLayer) -> f32 + '_)> {
        let gx = ((p.x - self.center.x) as f32 + self.half[0]) / self.cell;
        let gz = ((p.z - self.center.z) as f32 + self.half[1]) / self.cell;
        if gx < 0.0 || gz < 0.0 || gx > (self.nx - 1) as f32 || gz > (self.nz - 1) as f32 {
            return None;
        }
        let (i, j) = (
            (gx.floor() as usize).min(self.nx - 2),
            (gz.floor() as usize).min(self.nz - 2),
        );
        let (tx, tz) = (gx - i as f32, gz - j as f32);
        let h = |i: usize, j: usize| self.vertices[j * self.nx + i].position[1];
        let y = (h(i, j) * (1.0 - tx) + h(i + 1, j) * tx) * (1.0 - tz)
            + (h(i, j + 1) * (1.0 - tx) + h(i + 1, j + 1) * tx) * tz;
        let near = &self.vertices[(gz.round() as usize) * self.nx + gx.round() as usize];
        let weight = move |layer: GroundLayer| {
            self.slots
                .iter()
                .zip(near.weights)
                .filter(|(l, _)| **l == layer)
                .map(|(_, w)| w as f32 / 255.0)
                .sum()
        };
        Some((self.center.y + y as f64, weight))
    }
}

/// Grid points under keep-clear circles and covers.
fn bare_mask(region: &EcologyRegion, resolution: [u32; 2], covers: &[Cover]) -> Vec<bool> {
    let [rx, rz] = resolution;
    let size = Vec2::new(region.half_extents[0] * 2.0, region.half_extents[1] * 2.0);
    let step = Vec2::new(size.x / (rx - 1) as f32, size.y / (rz - 1) as f32);
    let corner = -size * 0.5;
    let index = |p: Vec2| -> (i64, i64) {
        (
            ((p.x - corner.x) / step.x).round() as i64,
            ((p.y - corner.y) / step.y).round() as i64,
        )
    };
    // Each marked over its own bounding box (not every point against every one: a region can
    // hold hundreds of rocks).
    let mut bare = vec![false; (rx * rz) as usize];
    let shapes = region
        .keep_clear
        .iter()
        .map(|c| Cover {
            a: Vec2::new(c[0], c[1]),
            b: Vec2::new(c[0], c[1]),
            radius: c[2],
        })
        .chain(covers.iter().copied());
    for c in shapes {
        let (lo, hi) = (c.a.min(c.b) - c.radius, c.a.max(c.b) + c.radius);
        let (i0, j0) = index(lo);
        let (i1, j1) = index(hi);
        let ab = c.b - c.a;
        for j in (j0 - 1).max(0)..=(j1 + 1).min(rz as i64 - 1) {
            for i in (i0 - 1).max(0)..=(i1 + 1).min(rx as i64 - 1) {
                let q = corner + Vec2::new(i as f32 * step.x, j as f32 * step.y);
                let t = ((q - c.a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
                if q.distance(c.a + ab * t) < c.radius {
                    bare[(j as u32 * rx + i as u32) as usize] = true;
                }
            }
        }
    }
    bare
}

/// The look of a region's blades: its grass species' colors and height.
pub fn region_style(library: &PlantLibrary, region: &EcologyRegion) -> Option<GrassStyle> {
    let grass = region
        .species
        .iter()
        .filter_map(|n| library.id(n))
        .map(|id| library.get(id))
        .find(|sp| sp.form == PlantForm::Grass)?;
    let lush = grass.leaf_color.map(|c| c * 0.62);
    let straw = [0.42, 0.34, 0.16];
    let dry = std::array::from_fn(|i| grass.accent_color[i] * 0.35 + straw[i] * 0.65);
    Some(GrassStyle {
        blades_per_m2: 420.0,
        height: grass.max_height_m * 0.7,
        height_variation: 0.45,
        width: 0.011,
        bend: 0.4,
        lush_color: lush,
        dry_color: dry,
        dryness: 0.2,
        wind_response: 1.0,
        draw_distance: 65.0,
        shadow_distance: 18.0,
        ..Default::default()
    })
}

/// Grid points along x and z for a region of `half_extents`.
fn resolution(half_extents: [f32; 2]) -> [u32; 2] {
    half_extents.map(|h| ((h * 2.0 / GRASS_CELL_M).ceil() as u32 + 1).clamp(2, MAX_RESOLUTION))
}

/// Something lying on the ground that grass doesn't grow under: a circle, or a segment
/// (a log) with a radius. Positions in meters from the region's center along world x/z.
#[derive(Debug, Clone, Copy)]
pub struct Cover {
    pub a: Vec2,
    pub b: Vec2,
    pub radius: f32,
}

/// Density (0..1) and dryness (0..1) maps for a region from its grass plants, soil and what
/// covers the ground. `plants` are (position from the region's center, cover radius,
/// vigor 0..1, health 0..1) of living grass.
#[allow(clippy::too_many_arguments)]
pub fn grass_maps(
    region: &EcologyRegion,
    soil: &Soil,
    center: DVec3,
    resolution: [u32; 2],
    plants: &[(Vec2, f32, f32, f32)],
    covers: &[Cover],
) -> (Vec<f32>, Vec<f32>) {
    let [rx, rz] = resolution;
    let size = Vec2::new(region.half_extents[0] * 2.0, region.half_extents[1] * 2.0);
    let step = Vec2::new(size.x / (rx - 1) as f32, size.y / (rz - 1) as f32);
    let corner = -size * 0.5;
    let n = (rx * rz) as usize;
    let mut cover = vec![0.0f32; n];
    let mut sick = vec![0.0f32; n];
    let mut weight = vec![0.0f32; n];
    let index = |p: Vec2| -> (i64, i64) {
        (
            ((p.x - corner.x) / step.x).round() as i64,
            ((p.y - corner.y) / step.y).round() as i64,
        )
    };
    for &(p, r, vigor, health) in plants {
        let reach = (r * 1.4).max(step.x.max(step.y));
        let (ci, cj) = index(p);
        let (si, sj) = (
            (reach / step.x).ceil() as i64,
            (reach / step.y).ceil() as i64,
        );
        for j in (cj - sj).max(0)..=(cj + sj).min(rz as i64 - 1) {
            for i in (ci - si).max(0)..=(ci + si).min(rx as i64 - 1) {
                let q = corner + Vec2::new(i as f32 * step.x, j as f32 * step.y);
                let d = q.distance(p) / reach;
                if d >= 1.0 {
                    continue;
                }
                let f = (1.0 - d * d) * vigor;
                let k = (j as u32 * rx + i as u32) as usize;
                cover[k] += f;
                sick[k] += f * (1.0 - health);
                weight[k] += f;
            }
        }
    }
    // Keep-clear circles and covers.
    let bare = bare_mask(region, resolution, covers);
    let mut density = Vec::with_capacity(n);
    let mut dryness = Vec::with_capacity(n);
    for j in 0..rz {
        for i in 0..rx {
            let k = (j * rx + i) as usize;
            let q = corner + Vec2::new(i as f32 * step.x, j as f32 * step.y);
            let world = center + DVec3::new(q.x as f64, 0.0, q.y as f64);
            let blocked = !soil.contains(world) || soil.blocked[soil.index(world)];
            // Meadows are patchy even where the tufts are thick: dense swards, thin rises and
            // bare gaps a few meters across, so the ground shows through here and there.
            let (x, z) = (q.x as f64, q.y as f64);
            let patch = fbm2(x * 0.15, z * 0.15, region.seed ^ 0x6a55, 3, 2.0, 0.5);
            let gaps = fbm2(x * 0.45, z * 0.45, region.seed ^ 0x9a9, 2, 2.0, 0.5);
            let d = if blocked || bare[k] {
                0.0
            } else {
                // Tufts overlap: 1.2 of cover is a closed sward.
                let sward = (cover[k] / 1.2).min(1.0).powf(0.8);
                let d = sward
                    * (0.3 + 0.75 * smoothstep(-0.6, 0.35, patch))
                    * (1.0 - 0.6 * smoothstep(0.15, 0.45, gaps));
                if d < 0.06 { 0.0 } else { d.min(1.0) }
            };
            density.push(d);
            let water = if soil.contains(world) {
                soil.sample(world)[0]
            } else {
                0.5
            };
            let thirst = ((0.45 - water) / 0.35).clamp(0.0, 1.0);
            let ill = if weight[k] > 0.0 {
                sick[k] / weight[k]
            } else {
                0.0
            };
            // Drier, paler grass on the thin patches.
            let parched = 0.12 * (1.0 - smoothstep(-0.6, 0.35, patch));
            dryness.push((0.1 + parched + 0.6 * thirst + 0.8 * ill).min(1.0));
        }
    }
    (density, dryness)
}

/// A region's grass maps being built on the async pool; the result lands in `slot`.
#[derive(Component)]
pub struct GrassBuild {
    slot: Arc<Mutex<Option<BuiltGrass>>>,
}

/// What a build produces, and what it was built for.
struct BuiltGrass {
    /// Each field's layer, look, density and dryness.
    layers: Vec<(FloorLayer, GrassStyle, Vec<f32>, Vec<f32>)>,
    /// Rebuilt only on the first build and after the terrain changed: the grass's (on the
    /// ground) and the floor layers' (on the forest floor's surface).
    heights: Option<Vec<f32>>,
    floor_heights: Option<Vec<f32>>,
    resolution: [u32; 2],
    size: [f32; 2],
    center: DVec3,
}

/// Run `job` on the async pool (inline where there is none, as in some tests) and put its
/// result in the returned slot.
fn spawn_build(job: impl FnOnce() -> BuiltGrass + Send + 'static) -> GrassBuild {
    let slot = Arc::new(Mutex::new(None));
    let out = slot.clone();
    let run = move || {
        let built = job();
        if let Ok(mut s) = out.lock() {
            *s = Some(built);
        }
    };
    match AsyncComputeTaskPool::try_get() {
        Some(pool) => pool.spawn(async move { run() }).detach(),
        None => run(),
    }
    GrassBuild { slot }
}

/// Spawn, update and remove each region's grass field. The maps are built off the frame (on
/// the async pool) from what the region holds now, and swapped in when ready.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn sync_region_grass(
    mut commands: Commands,
    mut timer: Local<f32>,
    mut relief_due: Local<bool>,
    time: Res<Time>,
    settings: Res<EcologySettings>,
    season: Res<FoliageSeason>,
    library: Res<PlantLibrary>,
    ground: Option<Res<Ground>>,
    mut terrain: MessageReader<engine_world::TerrainChanged>,
    regions: Query<(
        Entity,
        &EcologyRegion,
        &RegionState,
        Option<&RegionGrass>,
        Option<&RegionFloor>,
        Option<&GrassBuild>,
    )>,
    mut fields: Query<(&mut GrassField, &mut Transform, &GrassOf)>,
    plants: Query<(&Plant, &Transform, &InRegion), Without<GrassField>>,
    logs: Query<(&FelledLog, &Transform), (With<LogPlaced>, Without<GrassField>)>,
    rocks: Query<&Transform, (With<Rock>, Without<GrassField>)>,
    orphans: Query<(Entity, &GrassOf)>,
    (loading, surfaces): (
        Option<ResMut<crate::loading::FoliageLoading>>,
        Query<(Ref<GroundSurface>, &GlobalTransform)>,
    ),
) {
    // Finished builds: swap them in.
    for (region_e, _, _, linked, floor, build) in &regions {
        let Some(build) = build else { continue };
        let Some(built) = build.slot.lock().ok().and_then(|mut s| s.take()) else {
            continue;
        };
        commands.entity(region_e).remove::<GrassBuild>();
        if !settings.gpu_grass {
            continue;
        }
        let field_at = Transform::from_translation(built.center);
        let mut floor = floor.copied().unwrap_or_default();
        let had_floor = floor.herbs.is_some() || floor.litter.is_some();
        // A new field takes the grass field's heights when the build didn't bring them.
        let grass_heights = || {
            linked
                .and_then(|g| fields.get(g.0).ok())
                .map(|(f, _, _)| f.heights.clone())
        };
        let known_heights = built.heights.clone().or_else(grass_heights);
        let known_floor = built.floor_heights.clone().or_else(grass_heights);
        let mut wanted = [false; 2];
        for (layer, style, density, dryness) in built.layers {
            let slot = match layer {
                FloorLayer::Grass => None,
                FloorLayer::Herbs => Some(0),
                FloorLayer::Litter => Some(1),
            };
            if let Some(i) = slot {
                wanted[i] = true;
            }
            let existing = match slot {
                None => linked.map(|g| g.0),
                Some(0) => floor.herbs,
                Some(_) => floor.litter,
            };
            match existing.and_then(|e| fields.get_mut(e).ok()) {
                Some((mut field, mut t, _)) => {
                    let heights = match slot {
                        None => &built.heights,
                        Some(_) => &built.floor_heights,
                    };
                    if let Some(h) = heights {
                        field.heights = h.clone();
                    }
                    field.size = built.size;
                    field.resolution = built.resolution;
                    field.density = density;
                    field.dryness = dryness;
                    field.style = style;
                    if t.translation != field_at.translation {
                        *t = field_at;
                    }
                }
                None => {
                    if layer != FloorLayer::Grass {
                        let on = density.iter().filter(|d| **d > 0.0).count();
                        tracing::info!(
                            "foliage: forest floor {layer:?} on {:.0}% of a {:.0} x {:.0} m region",
                            100.0 * on as f32 / density.len().max(1) as f32,
                            built.size[0],
                            built.size[1],
                        );
                    }
                    let field = GrassField {
                        size: built.size,
                        resolution: built.resolution,
                        heights: match slot {
                            None => known_heights.clone(),
                            Some(_) => known_floor.clone(),
                        }
                        .unwrap_or_default(),
                        density,
                        dryness,
                        style,
                    };
                    let e = commands
                        .spawn((field_at, field, GrassOf(region_e), layer))
                        .id();
                    match slot {
                        None => {
                            commands.entity(region_e).insert(RegionGrass(e));
                        }
                        Some(0) => floor.herbs = Some(e),
                        Some(_) => floor.litter = Some(e),
                    }
                }
            }
        }
        // Layers the region no longer draws.
        for (i, slot) in [&mut floor.herbs, &mut floor.litter]
            .into_iter()
            .enumerate()
        {
            if !wanted[i]
                && let Some(e) = slot.take()
            {
                commands.entity(e).despawn();
            }
        }
        if floor.herbs.is_some() || floor.litter.is_some() || had_floor {
            commands.entity(region_e).insert(floor);
        }
    }

    *relief_due |= terrain.read().count() > 0;
    // The forest floor's surface built or rebuilt: the floor layers stand on it.
    *relief_due |= surfaces.iter().any(|(s, _)| s.is_changed());
    *timer -= time.real_delta_secs();
    // A region that has just come in gets its fields at once, not at the next round (and
    // loading waits for them).
    let unmapped =
        |(_, region, _, linked, _, _): &(_, &EcologyRegion, _, Option<&RegionGrass>, _, _)| {
            linked.is_none() && !floor_styles(&library, region).is_empty()
        };
    let fresh = settings.gpu_grass && regions.iter().any(|r| r.5.is_none() && unmapped(&r));
    if let Some(mut loading) = loading {
        let waiting = settings.gpu_grass && regions.iter().any(|r| unmapped(&r));
        if loading.floors_waiting != waiting {
            loading.floors_waiting = waiting;
        }
    }
    if *timer > 0.0 && !fresh && !*relief_due && !settings.is_changed() && !season.is_changed() {
        return;
    }
    *timer = REBUILD_SECS;
    // Fields whose region is gone.
    for (e, of) in &orphans {
        if !regions.contains(of.0) {
            commands.entity(e).despawn();
        }
    }
    let relief = std::mem::take(&mut *relief_due);
    let season_dry = grass_dryness(season.day_of_year);
    for (region_e, region, state, linked, floor, build) in &regions {
        let styles = if settings.gpu_grass {
            floor_styles(&library, region)
        } else {
            Vec::new()
        };
        if styles.is_empty() {
            if let Some(g) = linked {
                commands.entity(g.0).despawn();
                commands.entity(region_e).remove::<RegionGrass>();
            }
            if let Some(f) = floor {
                for e in [f.herbs, f.litter].into_iter().flatten() {
                    commands.entity(e).despawn();
                }
                commands.entity(region_e).remove::<RegionFloor>();
            }
            continue;
        }
        if build.is_some() {
            // Still building: the next round picks up what changed meanwhile.
            *relief_due |= relief;
            continue;
        }
        let res = resolution(region.half_extents);
        let center = state.center;
        let local = |t: &Transform| {
            Vec2::new(
                (t.translation.x - center.x) as f32,
                (t.translation.z - center.z) as f32,
            )
        };
        let inside = |p: Vec2| {
            p.x.abs() <= region.half_extents[0] + 1.0 && p.y.abs() <= region.half_extents[1] + 1.0
        };
        let crowns: Vec<Crown> = plants
            .iter()
            .filter(|(_, _, r)| r.0 == Some(region_e))
            .filter_map(|(p, t, _)| {
                let sp = library.get(p.species);
                let broadleaf = match sp.form {
                    PlantForm::Broadleaf | PlantForm::Shrub => true,
                    PlantForm::Conifer => false,
                    _ => return None,
                };
                let leafy = match p.stage {
                    PlantStage::Alive => 0.4 + 0.6 * p.size.min(1.0),
                    PlantStage::Dead { .. } => 0.25,
                    _ => return None,
                };
                let pos = local(t);
                inside(pos).then_some(Crown {
                    pos,
                    radius: p.crown_radius(sp),
                    leafy,
                    broadleaf,
                })
            })
            .collect();
        // Nothing grows in a trunk.
        let trunks: Vec<Cover> = plants
            .iter()
            .filter(|(p, _, r)| {
                r.0 == Some(region_e) && !matches!(p.stage, PlantStage::Burnt { .. })
            })
            .filter(|(p, _, _)| library.get(p.species).is_tree())
            .filter_map(|(p, t, _)| {
                let sp = library.get(p.species);
                let a = local(t);
                let r = sp.trunk_radius_m * p.size.max(0.15) + 0.12;
                inside(a).then_some(Cover { a, b: a, radius: r })
            })
            .collect();
        let grass: Vec<_> = plants
            .iter()
            .filter(|(p, _, r)| r.0 == Some(region_e) && p.stage.is_alive())
            .filter(|(p, _, _)| library.get(p.species).form == PlantForm::Grass)
            .map(|(p, t, _)| {
                let sp = library.get(p.species);
                (
                    local(t),
                    p.crown_radius(sp),
                    (0.35 + 0.65 * p.size).min(1.0),
                    p.health,
                )
            })
            .collect();
        // A log's transform sits at its butt (the model's base); the trunk runs up its axis.
        let mut covers: Vec<Cover> = logs
            .iter()
            .filter_map(|(log, t)| {
                let sp = library.get(log.species);
                let a = local(t);
                // Bare old logs have lost their thin tops (`meshgen::tree`).
                let reach = if log.years >= LOG_BARE_YEARS {
                    0.62
                } else {
                    0.85
                };
                let axis = t.rotation * Vec3::Y * (sp.max_height_m * t.scale.y * reach);
                let r = (sp.trunk_radius_m * log.size * 1.6).max(0.15);
                inside(a).then_some(Cover {
                    a,
                    b: a + Vec2::new(axis.x, axis.z),
                    radius: r,
                })
            })
            .collect();
        covers.extend(rocks.iter().filter_map(|t| {
            let p = local(t);
            let r = 0.45 * t.scale.x.max(t.scale.z);
            (inside(p) && r > 0.08).then_some(Cover {
                a: p,
                b: p,
                radius: r,
            })
        }));
        let existing = linked.and_then(|g| fields.get(g.0).ok());
        let need_heights =
            relief || existing.is_none_or(|(f, _, _)| f.heights.is_empty() || f.resolution != res);
        let shades = shades_floor(&styles);
        let ground = need_heights.then(|| ground.as_deref().cloned());
        let grids: Vec<CoverGrid> = if shades_floor(&styles) {
            surfaces
                .iter()
                .filter_map(|(s, gt)| CoverGrid::new(&s, gt.translation()))
                .collect()
        } else {
            Vec::new()
        };
        let region = region.clone();
        let soil = state.sim.soil.clone();
        let build = spawn_build(move || {
            let (density, mut dryness) = grass_maps(&region, &soil, center, res, &grass, &covers);
            // Autumn browns the meadow and winter leaves it straw, wherever it grows.
            for d in &mut dryness {
                *d += (1.0 - *d) * season_dry;
            }
            let floor = shades.then(|| {
                let mut covers = covers;
                covers.extend(trunks);
                floor_maps(
                    &region, &soil, center, res, &density, &crowns, &covers, &grids, season_dry,
                )
            });
            let heights = ground.map(|g| heights(g.as_ref(), center, &region, res));
            let floor_heights = heights
                .as_ref()
                .filter(|_| shades)
                .map(|h| onto_floor(h, &grids, center, &region, res));
            let mut maps = Some((density, dryness));
            let mut floor = floor.map(|[h, l]| [Some(h), Some(l)]);
            let layers = styles
                .into_iter()
                .filter_map(|(layer, style)| {
                    let (d, w) = match layer {
                        FloorLayer::Grass => maps.take()?,
                        FloorLayer::Herbs => floor.as_mut()?[0].take()?,
                        FloorLayer::Litter => floor.as_mut()?[1].take()?,
                    };
                    Some((layer, style, d, w))
                })
                .collect();
            BuiltGrass {
                layers,
                heights,
                floor_heights,
                resolution: res,
                size: [region.half_extents[0] * 2.0, region.half_extents[1] * 2.0],
                center,
            }
        });
        commands.entity(region_e).insert(build);
    }
}

/// Whether any of a region's fields is a forest floor layer.
fn shades_floor(styles: &[(FloorLayer, GrassStyle)]) -> bool {
    styles.iter().any(|(l, _)| *l != FloorLayer::Grass)
}

/// `heights` (on the ground) raised onto the forest floor's surface where a cover lies.
fn onto_floor(
    heights: &[f32],
    grids: &[CoverGrid],
    center: DVec3,
    region: &EcologyRegion,
    res: [u32; 2],
) -> Vec<f32> {
    let [rx, rz] = res;
    let size = [region.half_extents[0] * 2.0, region.half_extents[1] * 2.0];
    let mut out = heights.to_vec();
    for j in 0..rz {
        for i in 0..rx {
            let x = (i as f32 / (rx - 1) as f32 - 0.5) * size[0];
            let z = (j as f32 / (rz - 1) as f32 - 0.5) * size[1];
            let p = center + DVec3::new(x as f64, 0.0, z as f64);
            let k = (j * rx + i) as usize;
            if let Some(y) = grids.iter().find_map(|g| g.at(p).map(|(y, _)| y)) {
                // Never below the ground (a round cover dips under it at its edge).
                out[k] = out[k].max((y - center.y) as f32);
            }
        }
    }
    out
}

/// Ground heights over a region's grid, meters above its center.
fn heights(
    ground: Option<&Ground>,
    center: DVec3,
    region: &EcologyRegion,
    res: [u32; 2],
) -> Vec<f32> {
    let [rx, rz] = res;
    let size = [region.half_extents[0] * 2.0, region.half_extents[1] * 2.0];
    let mut out = Vec::with_capacity((rx * rz) as usize);
    for j in 0..rz {
        for i in 0..rx {
            let x = (i as f32 / (rx - 1) as f32 - 0.5) * size[0];
            let z = (j as f32 / (rz - 1) as f32 - 0.5) * size[1];
            let p = center + DVec3::new(x as f64, 2.0, z as f64);
            let h = ground
                .and_then(|g| g.height(p))
                .map_or(0.0, |h| (h - center.y) as f32);
            out.push(h);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FoliageLoading;
    use crate::plant::{Genes, RegionHistory};
    use engine_core::time::TimeSource;

    fn app() -> App {
        let mut app = App::new();
        app.world
            .resource_mut::<Time>()
            .set_source(TimeSource::Manual {
                step_secs: 1.0 / 60.0,
            });
        app.add_plugins(engine_world::WorldPlugin);
        app.add_plugins(crate::FoliagePlugin);
        app.world.insert_resource(crate::FoliageLoading::in_frame());
        app
    }

    /// Run until no region's maps are still building (they build on the async pool).
    fn settle(app: &mut App) {
        let start = std::time::Instant::now();
        loop {
            app.update();
            let building = app
                .world
                .query_filtered::<(), With<GrassBuild>>()
                .iter(&app.world)
                .count();
            if building == 0 || start.elapsed().as_secs() > 30 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    #[test]
    fn grass_grows_where_the_ecology_puts_it() {
        let mut app = app();
        let region = app
            .world
            .spawn((
                Transform::from_xyz(50.0, 0.0, 0.0),
                EcologyRegion {
                    half_extents: [20.0, 20.0],
                    species: vec!["grass".into(), "oak".into()],
                    keep_clear: vec![[10.0, 0.0, 3.0]],
                    history: RegionHistory {
                        grow_years: 0.0,
                        initial_plants: [0, 0, 0],
                        ..Default::default()
                    },
                    seed_rain: 0.0,
                    ..Default::default()
                },
            ))
            .id();
        let lib = PlantLibrary::default();
        let grass = lib.id("grass").unwrap();
        // A lawn on the -x half, none on the +x half.
        for i in 0..20 {
            for j in 0..40 {
                app.world.spawn((
                    Transform::from_xyz(50.0 - 19.0 + i as f64 * 0.9, 0.0, -19.0 + j as f64 * 0.95),
                    Plant::new(grass, 1.0, Genes::default()),
                ));
            }
        }
        // The maps are rebuilt every few seconds; give the plants time to join the region.
        app.update_n(200);
        settle(&mut app);
        let field_e = app
            .world
            .get::<RegionGrass>(region)
            .expect("a grass field")
            .0;
        let field = app.world.get::<GrassField>(field_e).unwrap();
        let [rx, rz] = field.resolution;
        assert_eq!(field.density.len(), (rx * rz) as usize);
        let at = |x: f32, z: f32| {
            let i = ((x / field.size[0] + 0.5) * (rx - 1) as f32).round() as u32;
            let j = ((z / field.size[1] + 0.5) * (rz - 1) as f32).round() as u32;
            field.density[(j * rx + i) as usize]
        };
        // The lawn is thick on the whole but patchy, with the odd gap.
        let lawn: Vec<f32> = (0..15)
            .flat_map(|i| (0..30).map(move |j| (-17.0 + i as f32, -15.0 + j as f32)))
            .map(|(x, z)| at(x, z))
            .collect();
        let mean = lawn.iter().sum::<f32>() / lawn.len() as f32;
        assert!(mean > 0.55, "lawn {mean}");
        assert!(lawn.iter().any(|&d| d > 0.9), "no dense sward");
        assert!(lawn.iter().any(|&d| d < 0.5), "no thin patch");
        assert!(at(14.0, 5.0) < 0.05, "bare {}", at(14.0, 5.0));
        assert_eq!(at(10.0, 0.0), 0.0, "keep-clear");

        // A log lying in the lawn along +z: no grass along its length, grass behind its butt.
        let oak = lib.id("oak").unwrap();
        app.world.spawn((
            Transform::from_xyz(50.0 - 10.0, 0.0, -10.0),
            FelledLog {
                species: oak,
                size: 0.8,
                ..Default::default()
            },
            crate::felling::LogPose {
                yaw: std::f32::consts::FRAC_PI_2,
                ..Default::default()
            },
        ));
        app.update_n(200);
        settle(&mut app);
        // It rests a little to the side of where it fell, its butt off the stump.
        let butt = app
            .world
            .query_filtered::<&Transform, With<FelledLog>>()
            .single(&app.world)
            .unwrap()
            .translation;
        let (x, z) = ((butt.x - 50.0) as f32, butt.z as f32);
        let field = app.world.get::<GrassField>(field_e).unwrap();
        let at = |x: f32, z: f32| {
            let i = ((x / field.size[0] + 0.5) * (rx - 1) as f32).round() as u32;
            let j = ((z / field.size[1] + 0.5) * (rz - 1) as f32).round() as u32;
            field.density[(j * rx + i) as usize]
        };
        let length = lib.get(oak).max_height_m * 0.8;
        for s in [0.1, 0.4, 0.7] {
            assert_eq!(at(x, z + length * s), 0.0, "under the log at {s}");
        }
        assert!(
            (1..4).any(|k| at(x, z - k as f32) > 0.3),
            "no grass behind the butt"
        );

        // Turned off: the field goes, and the tufts draw again.
        app.world.resource_mut::<EcologySettings>().gpu_grass = false;
        app.update_n(2);
        assert!(app.world.get::<RegionGrass>(region).is_none());
        assert!(app.world.get_entity(field_e).is_err());
    }

    #[test]
    fn a_forest_gets_herbs_in_the_shade_and_leaves_under_broadleaves() {
        let mut app = app();
        let region = app
            .world
            .spawn((Transform::default(), crate::demo::forest_region()))
            .id();
        app.update_n(300);
        settle(&mut app);
        let floor = *app
            .world
            .get::<RegionFloor>(region)
            .expect("a forest floor");
        let mut q = app.world.query::<(&GrassField, &FloorLayer, &GrassOf)>();
        let layers: Vec<FloorLayer> = q.iter(&app.world).map(|(_, l, _)| *l).collect();
        assert_eq!(layers.len(), 3, "{layers:?}");
        for (e, layer) in [
            (floor.herbs, FloorLayer::Herbs),
            (floor.litter, FloorLayer::Litter),
        ] {
            let (f, l, of) = q.get(&app.world, e.unwrap()).unwrap();
            assert_eq!((*l, of.0), (layer, region));
            let on = f.density.iter().filter(|d| **d > 0.0).count() as f32;
            let share = on / f.density.len() as f32;
            // Patches, not a carpet over everything.
            assert!((0.1..0.8).contains(&share), "{layer:?} covers {share}");
            assert_eq!(f.heights.len(), f.density.len());
        }
    }

    #[test]
    fn the_floor_layers_stand_on_the_forest_floor() {
        let mut app = app();
        app.world
            .init_resource::<engine_assets::Assets<engine_render::Mesh>>();
        app.world
            .init_resource::<engine_assets::Assets<engine_render::StandardMaterial>>();
        app.world
            .init_resource::<engine_assets::Assets<engine_render::Image>>();
        let mut queue = bevy_ecs::world::CommandQueue::default();
        crate::demo::spawn_demo(&mut Commands::new(&mut queue, &app.world), DVec3::ZERO);
        queue.apply(&mut app.world);
        // The floor's surface builds off the frame; the fields are mapped again once it's in.
        let start = std::time::Instant::now();
        loop {
            app.update();
            let built = app
                .world
                .query::<&GroundSurface>()
                .iter(&app.world)
                .next()
                .is_some();
            if (built && app.world.resource::<FoliageLoading>().scenery_building == 0)
                || start.elapsed().as_secs() > 180
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        app.update_n(3);
        settle(&mut app);
        let mut covers = app.world.query::<(&GroundSurface, &GlobalTransform)>();
        let grids: Vec<CoverGrid> = covers
            .iter(&app.world)
            .filter_map(|(s, gt)| CoverGrid::new(s, gt.translation()))
            .collect();
        assert!(!grids.is_empty(), "no forest floor");
        let mut q = app
            .world
            .query::<(&GrassField, &FloorLayer, &GlobalTransform)>();
        let mut checked = 0;
        for (f, layer, gt) in q.iter(&app.world) {
            let [rx, rz] = f.resolution;
            for (j, i) in [(rz / 2, rx / 2), (rz / 3, rx / 2), (rz / 2, rx / 3)] {
                let x = (i as f32 / (rx - 1) as f32 - 0.5) * f.size[0];
                let z = (j as f32 / (rz - 1) as f32 - 0.5) * f.size[1];
                let p = gt.translation() + DVec3::new(x as f64, 0.0, z as f64);
                let Some(y) = grids.iter().find_map(|g| g.at(p).map(|(y, _)| y)) else {
                    continue;
                };
                let h = gt.translation().y + f.heights[(j * rx + i) as usize] as f64;
                if *layer == FloorLayer::Grass {
                    // Grass grows from the ground, through the floor.
                    assert!(h < y, "grass at {h}, floor at {y}");
                } else {
                    assert!((h - y).abs() < 0.03, "{layer:?} at {h}, floor at {y}");
                }
                checked += 1;
            }
        }
        assert!(checked >= 6, "{checked}");
    }

    #[test]
    fn herbs_and_leaves_follow_the_crowns() {
        let region = EcologyRegion {
            half_extents: [20.0, 20.0],
            keep_clear: vec![[-10.0, -10.0, 2.5]],
            ..Default::default()
        };
        let soil = Soil::build(&region, DVec3::ZERO, &[], &crate::sim::FlatEnv::default());
        let res = resolution(region.half_extents);
        let n = (res[0] * res[1]) as usize;
        // Oaks over the -x half; pines over the +x half's -z quarter; open ground beyond.
        let mut crowns = Vec::new();
        for i in 0..5 {
            for j in 0..9 {
                let z = -20.0 + j as f32 * 5.0;
                crowns.push(Crown {
                    pos: Vec2::new(-19.0 + i as f32 * 4.0, z),
                    radius: 4.0,
                    leafy: 1.0,
                    broadleaf: true,
                });
                if z < 0.0 {
                    crowns.push(Crown {
                        pos: Vec2::new(3.0 + i as f32 * 4.0, z),
                        radius: 4.0,
                        leafy: 1.0,
                        broadleaf: false,
                    });
                }
            }
        }
        let trunk = Cover {
            a: Vec2::new(-12.0, 3.0),
            b: Vec2::new(-12.0, 3.0),
            radius: 0.5,
        };
        let maps = |grass: f32| {
            floor_maps(
                &region,
                &soil,
                DVec3::ZERO,
                res,
                &vec![grass; n],
                &crowns,
                &[trunk],
                &[],
                0.0,
            )
        };
        let [(herbs, _), (litter, _)] = maps(0.0);
        let at =
            |m: &[f32], x: i32, z: i32| m[((z + 20) as u32 * res[0] + (x + 20) as u32) as usize];
        let mean = |m: &[f32], xs: std::ops::Range<i32>, zs: std::ops::Range<i32>| {
            let v: Vec<f32> = xs
                .flat_map(|x| zs.clone().map(move |z| (x, z)))
                .map(|(x, z)| at(m, x, z))
                .collect();
            v.iter().sum::<f32>() / v.len() as f32
        };
        let oaks = (-16..-4, -16..16);
        let pines = (6..16, -16..-4);
        let open = (6..16, 6..16);
        assert!(
            mean(&herbs, oaks.0.clone(), oaks.1.clone()) > 0.15,
            "herbs under oaks"
        );
        assert!(
            mean(&herbs, pines.0.clone(), pines.1.clone()) > 0.15,
            "herbs under pines"
        );
        assert!(
            mean(&herbs, open.0.clone(), open.1.clone()) < 0.02,
            "herbs in the open"
        );
        assert!(
            mean(&litter, oaks.0.clone(), oaks.1.clone()) > 0.4,
            "leaves under oaks"
        );
        assert!(
            mean(&litter, pines.0.clone(), pines.1.clone()) < 0.02,
            "leaves under pines"
        );
        assert!(
            mean(&litter, open.0.clone(), open.1.clone()) < 0.02,
            "leaves in the open"
        );
        for m in [&herbs, &litter] {
            assert_eq!(at(m, -12, 3), 0.0, "in a trunk");
            assert_eq!(at(m, -10, -10), 0.0, "keep-clear");
        }
        // Thick grass crowds them out.
        let [(herbs_in_grass, _), _] = maps(1.0);
        assert!(
            mean(&herbs_in_grass, oaks.0.clone(), oaks.1.clone())
                < 0.3 * mean(&herbs, oaks.0.clone(), oaks.1.clone())
        );
    }

    #[test]
    fn caves_without_grass_get_no_field() {
        let lib = PlantLibrary::default();
        assert!(region_style(&lib, &crate::demo::cave_region()).is_none());
        assert!(region_style(&lib, &crate::demo::forest_region()).is_some());
    }
}
