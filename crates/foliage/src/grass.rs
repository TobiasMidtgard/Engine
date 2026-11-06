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

/// On a region's field: where its last rebuild changed it, so the ground-cover painter
/// repaints only there (and the field isn't touched at all when a rebuild changed nothing).
#[derive(Component, Debug, Clone, PartialEq)]
pub struct GrassEdit {
    /// Everything changed (a new field, size, resolution or style).
    pub all: bool,
    /// Rectangles of map points `[x0, z0, x1, z1)` (ends exclusive) whose density, dryness or
    /// kind changed: runs of [`EDIT_TILE`]-point tiles along each row of tiles. Empty when only
    /// the heights did, or changes too small to repaint for.
    pub points: Vec<[u32; 4]>,
    /// The maps as the ground cover was last painted from them, tile by tile, so small changes
    /// add up until they count.
    painted: CoverBaseline,
}

#[derive(Debug, Clone, Default, PartialEq)]
struct CoverBaseline {
    density: Vec<f32>,
    dryness: Vec<f32>,
    kinds: Vec<u8>,
}

impl Default for GrassEdit {
    fn default() -> Self {
        Self {
            all: true,
            points: Vec::new(),
            painted: CoverBaseline::default(),
        }
    }
}

impl GrassEdit {
    /// A new field's: painted whole, the next rebuild's changes counted from its maps.
    fn new_field(field: &GrassField) -> Self {
        Self {
            painted: CoverBaseline {
                density: field.density.clone(),
                dryness: field.dryness.clone(),
                kinds: field.kinds.clone(),
            },
            ..Default::default()
        }
    }
}

/// Map points along a side of the tiles a [`GrassEdit`] counts changes in. A living forest
/// changes a few hundred points scattered over a region every rebuild (plants grow, die and
/// sprout, logs fall and rot), so one rectangle around them all would be the whole field.
pub const EDIT_TILE: u32 = 4;

/// How far density or dryness may drift from what the ground cover was painted from before
/// its tile is repainted: the cover is a coarse reading of the grass (footsteps, cover for
/// AI, snow), and growth moves crown shade a little every rebuild.
const EDIT_EPSILON: f32 = 0.1;

/// The tiles where `(density, dryness, kinds)` drifted from `painted` on a grid of
/// `resolution` points, as [`GrassEdit::points`]; those tiles of `painted` take the new
/// values. `None` (and `painted` the new maps) where the maps don't line up.
fn changed_points(
    resolution: [u32; 2],
    painted: &mut CoverBaseline,
    new: (&[f32], &[f32], &[u8]),
) -> Option<Vec<[u32; 4]>> {
    let n = (resolution[0] * resolution[1]) as usize;
    let old = (
        &painted.density[..],
        &painted.dryness[..],
        &painted.kinds[..],
    );
    let fits = |m: (&[f32], &[f32], &[u8])| {
        [m.0.len(), m.1.len(), m.2.len()]
            .iter()
            .all(|&l| l == n || l == 0)
    };
    if old.0.len() != new.0.len()
        || old.1.len() != new.1.len()
        || old.2.len() != new.2.len()
        || !fits(new)
    {
        *painted = CoverBaseline {
            density: new.0.to_vec(),
            dryness: new.1.to_vec(),
            kinds: new.2.to_vec(),
        };
        return None;
    }
    let w = resolution[0].max(1) as usize;
    let [tx, tz] = resolution.map(|r| r.div_ceil(EDIT_TILE).max(1) as usize);
    let mut tiles = vec![false; tx * tz];
    let mut mark = |i: usize| {
        let (x, z) = (i % w / EDIT_TILE as usize, i / w / EDIT_TILE as usize);
        tiles[z * tx + x] = true;
    };
    for (i, (a, b)) in old.0.iter().zip(new.0).enumerate() {
        if (a - b).abs() > EDIT_EPSILON {
            mark(i);
        }
    }
    for (i, (a, b)) in old.1.iter().zip(new.1).enumerate() {
        if (a - b).abs() > EDIT_EPSILON {
            mark(i);
        }
    }
    for (i, (a, b)) in old.2.iter().zip(new.2).enumerate() {
        if a != b {
            mark(i);
        }
    }
    let t = EDIT_TILE;
    let mut runs = Vec::new();
    for z in 0..tz {
        let mut x = 0;
        while x < tx {
            if !tiles[z * tx + x] {
                x += 1;
                continue;
            }
            let first = x;
            while x < tx && tiles[z * tx + x] {
                x += 1;
            }
            let (z0, z1) = (z as u32 * t, ((z as u32 + 1) * t).min(resolution[1]));
            let (x0, x1) = (first as u32 * t, (x as u32 * t).min(resolution[0]));
            runs.push([x0, z0, x1, z1]);
        }
    }
    // The cover is painted again from the new values where it is repainted.
    for &[x0, z0, x1, z1] in &runs {
        for z in z0..z1 {
            let r = (z * resolution[0]) as usize;
            let (a, b) = (r + x0 as usize, r + x1 as usize);
            for (dst, src) in [(&mut painted.density, new.0), (&mut painted.dryness, new.1)] {
                if !src.is_empty() {
                    dst[a..b].copy_from_slice(&src[a..b]);
                }
            }
            if !new.2.is_empty() {
                painted.kinds[a..b].copy_from_slice(&new.2[a..b]);
            }
        }
    }
    Some(runs)
}

/// What a region's GPU field draws: its meadow grass, or a layer of a forest's floor.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FloorLayer {
    Grass,
    /// Low shade herbs in rosettes ([`GrassStyle::forest_herbs`]).
    Herbs,
    /// Fallen leaves ([`GrassStyle::leaf_litter`]).
    Litter,
    /// Fern crowns where the region's fern plants grow ([`GrassStyle::fern`]): fronds on the
    /// GPU instead of a mesh and an entity each.
    Ferns,
}

/// On a region: the entities drawing its forest floor's herbs and fallen leaves.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct RegionFloor {
    pub herbs: Option<Entity>,
    pub litter: Option<Entity>,
    pub ferns: Option<Entity>,
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
    if forms.contains(&PlantForm::Fern) {
        out.push((FloorLayer::Ferns, GrassStyle::fern()));
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

/// A forest floor's maps over the same grid as [`grass_maps`].
pub struct FloorMaps {
    /// Herbs' density and dryness.
    pub herbs: (Vec<f32>, Vec<f32>),
    /// Fallen leaves' density and dryness.
    pub litter: (Vec<f32>, Vec<f32>),
    /// Which herb grows at each point (`GrassField::kinds` of the herbs' field, looks from
    /// [`herb_styles`]).
    pub herb_kinds: Vec<u8>,
    /// How thick the dwarf shrubs grow where they won the herbs' layer (0 elsewhere): the
    /// meadow grass thins under them.
    pub shrubs: Vec<f32>,
    /// What lies at each point of the litter: 0 fallen leaves, 1 twigs among them
    /// ([`GrassStyle::twigs`]).
    pub litter_kinds: Vec<u8>,
}

/// The forest floor's kinds of herb, in the order [`FloorMaps::herb_kinds`] names them:
/// broad-leaved shade herbs, wood sorrel, sweet woodruff and wood anemone, the dwarf shrubs
/// (bilberry, lingonberry, heather) and bramble.
pub fn herb_styles() -> (GrassStyle, Vec<GrassStyle>) {
    (
        GrassStyle::forest_herbs(),
        vec![
            GrassStyle::wood_sorrel(),
            GrassStyle::woodruff(),
            GrassStyle::wood_anemone(),
            GrassStyle::bilberry(),
            GrassStyle::lingonberry(),
            GrassStyle::heather(),
            GrassStyle::bramble(),
        ],
    )
}

/// [`herb_styles`] on `day`: bilberry drops its leaves for winter and stands as bare green
/// twigs; heather's flower spikes are fresh mauve only in late summer and dry to russet,
/// staying on the shoots through winter, its foliage turning bronze in the cold
/// (https://en.wikipedia.org/wiki/Calluna_vulgaris); lingonberry stays evergreen, a little
/// darker in winter.
pub fn herb_styles_on(day: f32) -> (GrassStyle, Vec<GrassStyle>) {
    let (base, mut kinds) = herb_styles();
    let winter = crate::season::winter_dormancy(day);
    // Spring flowers: sorrel, woodruff, anemone (kinds 1-3).
    let flowering = crate::season::herbs_flowering(day);
    for ((style, f), most) in kinds.iter_mut().zip(flowering).zip([0.3, 0.35, 0.55]) {
        style.bloom += (most - style.bloom).max(0.0) * f;
        style.bloom_size *= 1.0 + 0.6 * f;
    }
    if let Some(a) = kinds.get_mut(2) {
        // Each flowering shoot has its whorl of leaves under the flower too.
        a.blades_per_m2 *= 1.0 + 0.4 * flowering[2];
        // Wood anemone yellows in June and is gone till March.
        let gone = crate::season::anemone_dormant(day);
        a.lush_color = mix3(a.lush_color, [0.2, 0.17, 0.05], gone.min(1.0));
        a.height *= 1.0 - 0.9 * gone;
        a.width *= 1.0 - 0.6 * gone;
        if gone > 0.9 {
            a.bloom = 0.0;
        }
    }
    let k = DWARF_SHRUB_KIND as usize - 1;
    let ripe = crate::season::berries_ripe(day);
    if let Some(b) = kinds.get_mut(k) {
        // Crimson in autumn, then bare.
        let red = crate::season::bilberry_red(day);
        // Wine red where moist, orange-brown where dry, clumps varying (not one crimson).
        b.lush_color = mix3(b.lush_color, [0.17, 0.035, 0.03], red);
        b.dry_color = mix3(b.dry_color, [0.17, 0.07, 0.03], red);
        b.clump_color += 0.2 * red;
        b.tip_tint = mix3(b.tip_tint, [1.0, 1.0, 1.0], red);
        let bare = crate::season::bilberry_bare(day);
        // Green twigs, whatever the season's dryness.
        let twig = [0.05, 0.12, 0.03];
        b.width += (0.006 - b.width) * bare;
        b.leaf *= 1.0 - bare;
        b.gloss = 0.35 - 0.2 * bare;
        b.lush_color = mix3(b.lush_color, twig, bare);
        b.dry_color = mix3(b.dry_color, twig, bare);
        b.tip_tint = mix3(b.tip_tint, [1.0, 1.0, 1.0], bare);
        // Blue-black berries hanging among the leaves in July and August.
        b.bloom = 0.22 * ripe[0] * (1.0 - bare);
        if bare > 0.5 {
            b.alternate_leaves = 0.0;
            b.bloom = 0.0;
        }
    }
    if let Some(l) = kinds.get_mut(k + 1) {
        // Red berries at the shoots' tips from August, many through the winter.
        l.bloom = 0.09 * ripe[1];
        l.lush_color = mix3(l.lush_color, [0.03, 0.06, 0.02], winter);
        l.dry_color = mix3(l.dry_color, [0.045, 0.06, 0.025], winter);
    }
    if let Some(r) = kinds.get_mut(BRAMBLE_KIND as usize - 1) {
        // White flowers in June and July, then berries red going black; semi-evergreen, the
        // leaves darkening and many falling in a hard winter.
        let [flowers, berries, black, red] = crate::season::bramble(day);
        let berry = mix3([0.32, 0.025, 0.03], [0.025, 0.018, 0.03], black);
        let share = flowers / (flowers + berries).max(1e-3);
        r.bloom = 0.15 * flowers + 0.2 * berries;
        r.bloom_color = mix3(berry, r.bloom_color, share);
        r.bloom_size *= 0.55 + 0.45 * share;
        r.lush_color = mix3(r.lush_color, [0.04, 0.045, 0.02], 0.7 * winter);
        // Wine red in patches from October, some canes staying green.
        r.lush_color = mix3(r.lush_color, [0.085, 0.02, 0.03], 0.55 * red);
        r.clump_color += 0.3 * red;
        r.alternate_leaves *= 1.0 - 0.35 * winter;
    }
    if let Some(h) = kinds.get_mut(k + 2) {
        let fresh = crate::meadow::heather_blooming(day);
        h.bloom_color = mix3([0.085, 0.045, 0.03], h.bloom_color, fresh);
        h.lush_color = mix3(h.lush_color, [0.045, 0.045, 0.025], winter);
        h.dry_color = mix3(h.dry_color, [0.06, 0.045, 0.032], winter);
    }
    (base, kinds)
}

/// Share of a grassland's blades standing dead on `day`. Grasses don't shed their dead
/// leaves: unmown grassland carries last year's straw into the new season (in unburned
/// tallgrass prairie the standing dead and litter outweigh the year's growth; Knapp &
/// Seastedt 1986), so a spring sward is a third straw until the new growth overtops it by
/// June. Through midsummer about a sixth of the blades are dead (older leaves die as new ones
/// grow), rising again as the grass ripens and dies back from August.
pub fn thatch(day: f32) -> f32 {
    let d = day.rem_euclid(crate::season::YEAR_DAYS);
    let spring = 1.0 - smoothstep(110.0, 165.0, d);
    let autumn = smoothstep(215.0, 290.0, d);
    0.16 + 0.2 * spring.max(autumn)
}

/// The meadow's look on `day`: [`thatch`]'s dead blades among the green ones, and in its
/// winter rest ([`crate::season::winter_dormancy`]) the dead leaves bleach from straw to a
/// weathered grey-buff, the yellow goes out of the tips, snow and rain flatten them into a
/// matted, half-lying mat and break most seed heads off.
pub fn winter_grass(style: &mut GrassStyle, day: f32) {
    style.dead = thatch(day);
    let w = crate::season::winter_dormancy(day);
    if w <= 0.0 {
        return;
    }
    style.lying = style.lying.max(0.55 * w);
    style.height *= 1.0 - 0.15 * w;
    style.dry_color = mix3(style.dry_color, [0.21, 0.175, 0.12], w);
    style.lush_color = mix3(style.lush_color, [0.07, 0.08, 0.04], 0.6 * w);
    style.tip_tint = mix3(style.tip_tint, [1.12, 1.08, 1.0], w);
    style.gloss *= 1.0 - 0.6 * w;
    style.bloom *= 1.0 - 0.75 * w;
}

fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * t)
}

/// Which herb takes a spot of forest floor, from how closed the `canopy` is over it, how much
/// of that is broadleaved (`broad`), the soil's `water`, the needles painted on it and its
/// colonies (`patch`: one noise per kind, about -1..1). The ground flora of European woods
/// (Ellenberg's indicator values, e.g. https://en.wikipedia.org/wiki/Ellenberg%27s_indicator_values;
/// Plantlife and Woodland Trust species notes): wood sorrel carpets deep moist shade under
/// spruce and beech, on acid soil that needles make; woodruff grows in colonies on rich moist
/// soil under beech and oak; wood anemone in lighter broadleaf woods; other broad shade herbs
/// fill in between. `anemone_season` ([`anemone_season`]) weighs wood anemone by the day: its
/// carpets own the floor while it flowers and leave it to the others once it has gone to
/// ground.
pub fn herb_kind(
    canopy: f32,
    broad: f32,
    water: f32,
    needles: f32,
    patch: [f32; 4],
    anemone_season: f32,
) -> u8 {
    let moist = smoothstep(0.2, 0.45, water);
    let weights = [
        0.45,
        moist * (0.5 + 0.5 * canopy) * (0.6 + 0.6 * (1.0 - broad) + 0.4 * needles),
        broad * smoothstep(0.25, 0.5, water) * canopy * (1.0 - needles) * 1.1,
        // (Wood anemone flowers and leafs before the canopy closes: shade from above matters
        // less to it than to the others, needles more.)
        broad
            * (1.0 - 0.35 * canopy)
            * (0.6 + 0.4 * moist)
            * (1.0 - 0.7 * needles)
            * anemone_season,
    ];
    let mut best = (0u8, f32::MIN);
    for (i, (w, n)) in weights.iter().zip(patch).enumerate() {
        let w = w * (1.0 + 0.9 * n);
        if w > best.1 {
            best = (i as u8, w);
        }
    }
    best.0
}

/// How much wood anemone's spots of floor show it on `day` (for [`herb_kind`]): 1 most of the
/// spring, up to 2.5 while it flowers, 0.15 while it rests underground.
pub fn anemone_season(day: f32) -> f32 {
    let flowering = crate::season::herbs_flowering(day)[2];
    (1.0 + 1.5 * flowering) * (1.0 - 0.85 * crate::season::anemone_dormant(day))
}

/// The first of the dwarf shrubs among [`herb_styles`]' kinds (bilberry, lingonberry, heather).
pub const DWARF_SHRUB_KIND: u8 = 4;
/// Bramble among [`herb_styles`]' kinds.
pub const BRAMBLE_KIND: u8 = 7;

/// How much bramble wants a spot of floor, 0..1, from the canopy over it (`canopy`, the
/// broadleaved part `broad`), the soil's `water` and `nutrients` and the needles painted on
/// it. Bramble thickets fill broadleaf woods' lighter shade, and the gaps, clearings and
/// felled ground in them, where it spreads fast once the canopy opens; on fresh, moderately
/// rich soil (Ellenberg light 6, moisture 5-6, nitrogen 6). In deep shade it lingers as weak
/// sterile canes; it shuns waterlogged ground and the acid floor under conifers (Taylor 2005,
/// Biological Flora of the British Isles: Rubus fruticosus L. agg., J. Ecol. 93; Rodwell
/// 1991, British Plant Communities 1, W8 and W10 woodlands; Harmer 1999 on bramble after
/// felling in lowland broadleaf woods).
pub fn bramble_want(canopy: f32, broad: f32, water: f32, nutrients: f32, needles: f32) -> f32 {
    // Broadleaved share of what shades the spot; an opening counts as the wood around it.
    let share = if canopy > 0.1 { broad / canopy } else { 1.0 };
    (1.0 - 0.75 * smoothstep(0.7, 1.3, canopy))
        * smoothstep(0.35, 0.75, share)
        * smoothstep(0.18, 0.32, water)
        * (1.0 - smoothstep(0.6, 0.78, water))
        * (0.5 + 0.5 * smoothstep(0.25, 0.6, nutrients))
        * (1.0 - 0.8 * needles)
}

/// How much each dwarf shrub (bilberry, lingonberry, heather) wants a spot of floor, 0..1, from
/// the canopy over it (`canopy`, the needle-leaved part of it `conifer`), the soil's `water`
/// and the needles painted on it. The heath shrubs of acid soils (Ellenberg reaction values
/// 1-2; the boreal and Atlantic forest floors of Kielland-Lund 1981 and Rodwell's British Plant
/// Communities W18 and H9): bilberry carpets moist acid floors under spruce and pine where some
/// light comes through, lingonberry the drier ones under pine, heather open dry ground (pine
/// heath, ridges, burnt ground) and gives way as the canopy closes.
pub fn dwarf_shrubs(canopy: f32, conifer: f32, water: f32, needles: f32) -> [f32; 3] {
    let acid = smoothstep(0.1, 0.5, conifer).max(needles);
    let dry = 1.0 - smoothstep(0.2, 0.42, water);
    [
        acid * smoothstep(0.18, 0.32, water) * (1.0 - smoothstep(0.85, 1.4, canopy)),
        acid * dry * smoothstep(0.05, 0.3, canopy) * 0.9,
        (1.0 - smoothstep(0.25, 0.65, canopy)) * dry * (0.4 + 0.6 * acid),
    ]
}

/// Herbs' and fallen leaves' maps over the same grid as [`grass_maps`], from the crowns
/// overhead, the soil and the grass's own `grass` density, and which herb grows where.
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
    day: f32,
) -> FloorMaps {
    let [rx, rz] = resolution;
    let size = Vec2::new(region.half_extents[0] * 2.0, region.half_extents[1] * 2.0);
    let step = Vec2::new(size.x / (rx - 1) as f32, size.y / (rz - 1) as f32);
    let corner = -size * 0.5;
    let n = (rx * rz) as usize;
    let (canopy, broad) = canopy_maps(region, resolution, crowns);
    let fall = leaf_fall_map(region, resolution, crowns);
    // Woodland within about 8 m: a gap or clearing among trees, not open meadow.
    let wooded = spread(
        &canopy,
        resolution,
        (8.0 / step.x.min(step.y)).round() as i64,
    );
    let bare = bare_mask(region, resolution, covers, true);
    // Leaves blow onto the cleared trail too: only logs, rocks and trunks keep them off.
    let covered = bare_mask(region, resolution, covers, false);
    let mut herbs = (Vec::with_capacity(n), Vec::with_capacity(n));
    let mut litter = (Vec::with_capacity(n), Vec::with_capacity(n));
    let mut herb_kinds = Vec::with_capacity(n);
    let mut shrubs = Vec::with_capacity(n);
    let mut litter_kinds = Vec::with_capacity(n);
    let anemone = anemone_season(day);
    for j in 0..rz {
        for i in 0..rx {
            let k = (j * rx + i) as usize;
            let q = corner + Vec2::new(i as f32 * step.x, j as f32 * step.y);
            let world = center + DVec3::new(q.x as f64, 0.0, q.y as f64);
            let inside = soil.contains(world);
            let blocked = !inside || soil.blocked[soil.index(world)];
            // On the trail's tread (its brushed corridor and the margin kept clear beside it)
            // nothing roots, but leaves blow onto it.
            let on_trail = inside
                && region
                    .corridors
                    .iter()
                    .any(|c| q.distance(Vec2::new(c[0], c[1])) < c[2] + 0.6);
            let water = if inside { soil.sample(world)[0] } else { 0.5 };
            let (x, z) = (q.x as f64, q.y as f64);
            let open = 1.0 - 0.85 * grass.get(k).copied().unwrap_or(0.0);
            // What the floor is painted with here (none outside the covers: no say).
            let painted = surfaces.iter().find_map(|c| {
                c.at(world).map(|(_, w)| {
                    let leafy = w(GroundLayer::Litter) + 0.6 * w(GroundLayer::Humus);
                    let hostile = w(GroundLayer::Mud) + w(GroundLayer::Rock) + w(GroundLayer::Ash);
                    // A conifer wood's moss carpet lies on the same acid needle humus.
                    let acid = w(GroundLayer::Needles) + 0.8 * w(GroundLayer::Moss);
                    (leafy, hostile, acid.min(1.0))
                })
            });
            let (leafy, hostile, needles) = painted.unwrap_or((1.0, 0.0, 0.0));
            // Herbs spread in colonies a few meters across, under a closing canopy, thicker
            // where the soil stays moist.
            let colony = fbm2(x * 0.22, z * 0.22, region.seed ^ 0x4e8b, 3, 2.0, 0.5);
            let herb_d = if blocked || bare[k] {
                0.0
            } else {
                let d = smoothstep(0.2, 0.8, canopy[k])
                    * smoothstep(-0.35, 0.2, colony)
                    * (0.55 + 0.45 * smoothstep(0.15, 0.45, water))
                    * open
                    * (1.0 - hostile).max(0.0)
                    * (1.0 - 0.5 * needles);
                if d < 0.08 { 0.0 } else { d.min(1.0) }
            };
            // Dwarf shrubs carpet acid ground in sweeps tens of meters across, thinning where
            // grass has the ground; they win the spot where they want it more than the herbs.
            let shrub_w = dwarf_shrubs(canopy[k], (canopy[k] - broad[k]).max(0.0), water, needles);
            let sweep = fbm2(x * 0.06, z * 0.06, region.seed ^ 0x5d1b, 3, 2.0, 0.5);
            let shrub_patch = [0x55u64, 0x56, 0x57]
                .map(|salt| fbm2(x * 0.07, z * 0.07, region.seed ^ (salt << 8), 2, 2.0, 0.5));
            let (shrub, shrub_d) = if blocked || bare[k] || on_trail {
                (0, 0.0)
            } else {
                let (i, w) = shrub_w
                    .iter()
                    .zip(shrub_patch)
                    .map(|(w, n)| w * (1.0 + 0.8 * n))
                    .enumerate()
                    .fold((0, f32::MIN), |b, (i, w)| if w > b.1 { (i, w) } else { b });
                let d = w.min(1.0)
                    * smoothstep(-0.45, 0.05, sweep)
                    * (1.0 - 0.3 * grass.get(k).copied().unwrap_or(0.0))
                    * (1.0 - hostile).max(0.0);
                (i as u8, if d < 0.12 { 0.0 } else { d.min(1.0) })
            };
            // Bramble in thickets several meters across where it wants the ground.
            let thicket = fbm2(x * 0.08, z * 0.08, region.seed ^ 0x7b1e, 3, 2.0, 0.5);
            let bramble_d = if blocked || bare[k] || on_trail {
                0.0
            } else {
                let nutrients = if inside { soil.sample(world)[1] } else { 0.4 };
                let d = bramble_want(canopy[k], broad[k], water, nutrients, needles)
                    * smoothstep(0.03, 0.12, wooded[k])
                    * smoothstep(-0.15, 0.2, thicket)
                    * (1.0 - 0.4 * grass.get(k).copied().unwrap_or(0.0))
                    * (1.0 - hostile).max(0.0)
                    * 2.5;
                if d < 0.25 { 0.0 } else { d.min(1.0) }
            };
            let (shrub, shrub_d) = if bramble_d > shrub_d.max(herb_d) {
                (BRAMBLE_KIND - DWARF_SHRUB_KIND, bramble_d)
            } else {
                (shrub, shrub_d)
            };
            let herb = herb_d.max(shrub_d);
            let thirst = ((0.4 - water) / 0.3).clamp(0.0, 1.0);
            // Each herb's colonies, some meters across.
            let patch = [0x51u64, 0x52, 0x53, 0x54]
                .map(|salt| fbm2(x * 0.09, z * 0.09, region.seed ^ (salt << 8), 2, 2.0, 0.5));
            shrubs.push(if shrub_d > herb_d { shrub_d } else { 0.0 });
            herb_kinds.push(if shrub_d > herb_d {
                DWARF_SHRUB_KIND + shrub
            } else if herb > 0.0 {
                herb_kind(canopy[k], broad[k], water, needles, patch, anemone)
            } else {
                0
            });
            herbs.0.push(herb);
            herbs
                .1
                .push((0.05 + 0.6 * thirst + 0.5 * season_dry).min(1.0));
            // Fallen leaves lie thickest under broadleaf crowns, drifted into patches, and
            // fewer where grass or herbs cover the ground.
            let drift = fbm2(x * 0.3, z * 0.3, region.seed ^ 0x1177, 3, 2.0, 0.5);
            // Past a round forest's ragged edge nothing roots, but the edge trees' leaves
            // blow out over the ground there too ([`leaf_fall_map`]).
            let past_edge =
                blocked && crate::sim::past_round_edge(region, center, world.x, world.z);
            let leaves =
                if (blocked && !on_trail && !past_edge) || covered[k] || (bare[k] && !on_trail) {
                    0.0
                } else {
                    // Leaves fall where broadleaf crowns spread and drift a few meters past them
                    // ([`leaf_fall_map`]), so an opening or a path among them is littered too.
                    let d = smoothstep(0.1, 0.7, broad[k].max(fall[k]))
                        * (0.55 + 0.45 * smoothstep(-0.4, 0.4, drift))
                        * open
                        * (1.0 - 0.5 * herb)
                        * smoothstep(0.15, 0.6, leafy);
                    if d < 0.06 { 0.0 } else { d.min(1.0) }
                };
            // Dryness picks the leaf's color, fresh brown to pale ochre, in patches.
            let hue = fbm2(x * 0.9, z * 0.9, region.seed ^ 0x2c3, 2, 2.0, 0.5);
            // Twigs lie in spots among the leaves, more of them under a closed canopy (the
            // crowns shed them: ref3_forest_floor's sticks).
            let twig_spot = unit_hash(region.seed ^ 0x7719, i, j);
            litter_kinds.push(u8::from(
                leaves > 0.0 && twig_spot < 0.15 + 0.25 * canopy[k],
            ));
            litter.0.push(leaves);
            litter
                .1
                .push((0.25 + 0.5 * smoothstep(-0.35, 0.45, hue) + 0.2 * season_dry).min(1.0));
        }
    }
    FloorMaps {
        herbs,
        litter,
        herb_kinds,
        shrubs,
        litter_kinds,
    }
}

/// `map` averaged over a square `r` cells out each way (a separable box blur, clamped at the
/// edges): how much of something lies around each point.
fn spread(map: &[f32], resolution: [u32; 2], r: i64) -> Vec<f32> {
    let [rx, rz] = [resolution[0] as i64, resolution[1] as i64];
    let pass = |src: &[f32], horizontal: bool| -> Vec<f32> {
        let mut out = vec![0.0f32; src.len()];
        for j in 0..rz {
            for i in 0..rx {
                let (mut sum, mut n) = (0.0f32, 0.0f32);
                for d in -r..=r {
                    let (a, b) = if horizontal { (i + d, j) } else { (i, j + d) };
                    if a >= 0 && b >= 0 && a < rx && b < rz {
                        sum += src[(b * rx + a) as usize];
                        n += 1.0;
                    }
                }
                out[(j * rx + i) as usize] = sum / n.max(1.0);
            }
        }
        out
    };
    pass(&pass(map, true), false)
}

/// Whether `q` (meters from the region's center) lies on a trail's shoulder: beside its tread
/// (a brushed corridor is the tread and a foot each side, [`EcologyRegion::corridors`]) and kept
/// clear only by the trail's own circles, not by a pond's or a clearing's.
fn trail_shoulder(region: &EcologyRegion, q: Vec2) -> bool {
    let mut near = false;
    for c in &region.corridors {
        let d = q.distance(Vec2::new(c[0], c[1]));
        if d < c[2] - 0.2 {
            return false;
        }
        near |= d < c[2] + 0.6;
    }
    near && region.keep_clear.iter().all(|k| {
        q.distance(Vec2::new(k[0], k[1])) >= k[2]
            || region
                .corridors
                .iter()
                .any(|c| (c[0] - k[0]).abs() < 1e-3 && (c[1] - k[1]).abs() < 1e-3)
    })
}

/// A number in 0..1 that looks random, the same for the same `seed` and grid point.
fn unit_hash(seed: u64, i: u32, j: u32) -> f32 {
    let mut h = seed
        ^ (i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
        ^ (j as u64).wrapping_mul(0xc2b2_ae3d_27d4_eb4f);
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    (h >> 40) as f32 / (1u64 << 24) as f32
}

/// Where broadleaf crowns drop their leaves: under each crown and drifting past its edge,
/// fewer the further out (litterfall models spread each tree's leaves with a kernel that
/// decays over meters beyond the crown: Ferrari and Sugita 1996; Staelens et al. 2004, mixed
/// deciduous forest; Jonard et al. 2006, beech-oak mixtures). Out to 1.8 crown radii and
/// 2 m more; under the crown as much as `canopy_maps`' broadleaf share.
fn leaf_fall_map(region: &EcologyRegion, resolution: [u32; 2], crowns: &[Crown]) -> Vec<f32> {
    let [rx, rz] = resolution;
    let size = Vec2::new(region.half_extents[0] * 2.0, region.half_extents[1] * 2.0);
    let step = Vec2::new(size.x / (rx - 1) as f32, size.y / (rz - 1) as f32);
    let corner = -size * 0.5;
    let mut fall = vec![0.0f32; (rx * rz) as usize];
    for c in crowns.iter().filter(|c| c.broadleaf) {
        let reach = (c.radius * 1.8 + 2.0).max(step.x.max(step.y));
        let ci = ((c.pos.x - corner.x) / step.x).round() as i64;
        let cj = ((c.pos.y - corner.y) / step.y).round() as i64;
        let (si, sj) = (
            (reach / step.x).ceil() as i64,
            (reach / step.y).ceil() as i64,
        );
        for j in (cj - sj).max(0)..=(cj + sj).min(rz as i64 - 1) {
            for i in (ci - si).max(0)..=(ci + si).min(rx as i64 - 1) {
                let q = corner + Vec2::new(i as f32 * step.x, j as f32 * step.y);
                let d = q.distance(c.pos);
                if d >= reach {
                    continue;
                }
                // The crown's own share under it, then the drift tailing off to `reach`.
                let under = 1.0 - (d / c.radius.max(0.1)).powi(2);
                let past = 1.0 - (d / reach);
                let f = under.max(0.6 * past * past) * c.leafy;
                fall[(j as u32 * rx + i as u32) as usize] += f;
            }
        }
    }
    fall
}

/// How closed the canopy is over each grid point, and how much of it is broadleaved.
fn canopy_maps(
    region: &EcologyRegion,
    resolution: [u32; 2],
    crowns: &[Crown],
) -> (Vec<f32>, Vec<f32>) {
    let [rx, rz] = resolution;
    let size = Vec2::new(region.half_extents[0] * 2.0, region.half_extents[1] * 2.0);
    let step = Vec2::new(size.x / (rx - 1) as f32, size.y / (rz - 1) as f32);
    let corner = -size * 0.5;
    let n = (rx * rz) as usize;
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
    (canopy, broad)
}

/// Which grass grows at each grid point ([`crate::meadow`]): from the soil's water, the
/// canopy's shade and its edge, trampling and the verges of the paths kept clear, and ground
/// opened by fire or rotting wood.
pub fn grass_kinds(
    region: &EcologyRegion,
    soil: &Soil,
    center: DVec3,
    resolution: [u32; 2],
    crowns: &[Crown],
) -> Vec<u8> {
    let [rx, rz] = resolution;
    let size = [region.half_extents[0] * 2.0, region.half_extents[1] * 2.0];
    let (canopy, _) = canopy_maps(region, resolution, crowns);
    let origin = [
        center.x - size[0] as f64 * 0.5,
        center.z - size[1] as f64 * 0.5,
    ];
    let mut k = 0usize;
    crate::meadow::paint_kinds(resolution, size, origin, region.seed, |x, z| {
        let at = k;
        k += 1;
        let q = Vec2::new(x - size[0] * 0.5, z - size[1] * 0.5);
        let world = center + DVec3::new(q.x as f64, 0.0, q.y as f64);
        if !soil.contains(world) {
            return None;
        }
        let i = soil.index(world);
        // A path's verge: trodden turf a meter or so either side of a cleared circle's edge
        // (people walk the tread, and step off it only so far).
        let verge = region
            .keep_clear
            .iter()
            .chain(&region.corridors)
            .map(|c| {
                let d = q.distance(Vec2::new(c[0], c[1])) - c[2];
                1.0 - smoothstep(0.0, 1.5, d.abs())
            })
            .fold(0.0f32, f32::max);
        let shade = canopy.get(at).copied().unwrap_or(0.0).min(1.0);
        Some(crate::meadow::Site {
            water: soil.water[i],
            shade,
            slope: 0.0,
            sunny: 0.0,
            wear: soil.trample[i].max(verge),
            edge: smoothstep(0.05, 0.3, shade) * (1.0 - smoothstep(0.55, 0.9, shade)),
            opened: smoothstep(0.4, 0.6, soil.bonus[i]),
        })
    })
    .into_iter()
    .take((rx * rz) as usize)
    .collect()
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
fn bare_mask(
    region: &EcologyRegion,
    resolution: [u32; 2],
    covers: &[Cover],
    clearings: bool,
) -> Vec<bool> {
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
        .filter(|_| clearings)
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
        // To 150 m: the survivors of the thinning widen to keep the cover (Ghost of
        // Tsushima's grass; engine-design/fields-and-forests-look.md, 4.1).
        draw_distance: 150.0,
        shadow_distance: 18.0,
        clump_size: 0.6,
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

/// Fern cover above which a map point grows the meadow's fern kind.
const FERN_COVER: f32 = 0.3;

/// How thickly `plants` (position from the region's center, cover radius, vigor, health)
/// cover each map point: each adds up to its vigor at its middle, falling to 0 at 1.4 times
/// its radius, as [`grass_maps`] counts tufts.
pub fn plant_cover(
    region: &EcologyRegion,
    resolution: [u32; 2],
    plants: &[(Vec2, f32, f32, f32)],
) -> Vec<f32> {
    let [rx, rz] = resolution;
    let size = Vec2::new(region.half_extents[0] * 2.0, region.half_extents[1] * 2.0);
    let step = Vec2::new(size.x / (rx - 1) as f32, size.y / (rz - 1) as f32);
    let corner = -size * 0.5;
    let mut cover = vec![0.0f32; (rx * rz) as usize];
    for &(p, r, vigor, _) in plants {
        let reach = (r * 1.4).max(step.x.max(step.y));
        let ci = ((p.x - corner.x) / step.x).round() as i64;
        let cj = ((p.y - corner.y) / step.y).round() as i64;
        let (si, sj) = (
            (reach / step.x).ceil() as i64,
            (reach / step.y).ceil() as i64,
        );
        for j in (cj - sj).max(0)..=(cj + sj).min(rz as i64 - 1) {
            for i in (ci - si).max(0)..=(ci + si).min(rx as i64 - 1) {
                let q = corner + Vec2::new(i as f32 * step.x, j as f32 * step.y);
                let d = q.distance(p) / reach;
                if d < 1.0 {
                    cover[(j as u32 * rx + i as u32) as usize] += (1.0 - d * d) * vigor;
                }
            }
        }
    }
    cover
}

/// The fern layer's look on `day`: dead fronds fall over and lie on the floor from November
/// to May ([`crate::season::fern_collapse`]), where snow buries them.
pub fn winter_ferns(style: &mut GrassStyle, day: f32) {
    let c = crate::season::fern_collapse(day);
    style.lying = 0.85 * c;
    style.height *= 1.0 - 0.2 * c;
}

/// Density and dryness maps of a region's fern layer: thick where crowns crowd, none on paths
/// and under logs and rocks, rust brown as the season turns (`season_dry`).
pub fn fern_maps(
    region: &EcologyRegion,
    resolution: [u32; 2],
    ferns: &[(Vec2, f32, f32, f32)],
    covers: &[Cover],
    season_dry: f32,
) -> (Vec<f32>, Vec<f32>) {
    let cover = plant_cover(region, resolution, ferns);
    let bare = bare_mask(region, resolution, covers, true);
    let density = cover
        .iter()
        .zip(&bare)
        .map(|(&c, &b)| {
            if b || c < FERN_COVER {
                0.0
            } else {
                smoothstep(FERN_COVER, 1.0, c).max(0.25)
            }
        })
        .collect();
    let dryness = vec![0.08 + 0.92 * season_dry; cover.len()];
    (density, dryness)
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
    let bare = bare_mask(region, resolution, covers, true);
    let covered = bare_mask(region, resolution, covers, false);
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
    // The shoulders, where no tuft roots (`blocked`) but the sward beside them creeps over:
    // short, dense trodden turf up to the tread's edge (`grass_kinds` makes it the Turf kind),
    // as on a trail people walk, not a strip of bare soil.
    let reach = (2.5 / step.x.min(step.y)).ceil() as i64;
    let mut shoulders = Vec::new();
    for j in 0..rz {
        for i in 0..rx {
            let q = corner + Vec2::new(i as f32 * step.x, j as f32 * step.y);
            let k = (j * rx + i) as usize;
            if !covered[k] && trail_shoulder(region, q) {
                shoulders.push((i as i64, j as i64, k));
            }
        }
    }
    let lying = density.clone();
    for (i, j, k) in shoulders {
        let mut most = 0.0f32;
        for dj in -reach..=reach {
            for di in -reach..=reach {
                let (a, b) = (i + di, j + dj);
                if a < 0 || b < 0 || a >= rx as i64 || b >= rz as i64 {
                    continue;
                }
                most = most.max(lying[(b as u32 * rx + a as u32) as usize]);
            }
        }
        density[k] = density[k].max(0.85 * most);
    }
    (density, dryness)
}

/// A region's grass maps being built on the async pool; the result lands in `slot`.
#[derive(Component)]
pub struct GrassBuild {
    slot: Arc<Mutex<Option<BuiltGrass>>>,
}

/// What a build produces, and what it was built for.
/// The kinds maps of a region's fields.
struct FloorKinds {
    /// The meadow's kinds of grass and their looks (`GrassField::kinds`, `styles`).
    kinds: Vec<u8>,
    kind_styles: Vec<GrassStyle>,
    /// Which herb grows where on the forest floor (the herbs' field's `kinds`), and where
    /// twigs lie among the fallen leaves (the litter's).
    herb_kinds: Vec<u8>,
    /// The herbs' looks on the build's day ([`herb_styles_on`]).
    herb_styles: Vec<GrassStyle>,
    litter_kinds: Vec<u8>,
}

struct BuiltGrass {
    /// Each field's layer, look, density and dryness.
    layers: Vec<(FloorLayer, GrassStyle, Vec<f32>, Vec<f32>)>,
    /// What grows where in each layer.
    floor_kinds: FloorKinds,
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
    // (Terrain relief changed; season or settings changed) since the last build started.
    mut due: Local<(bool, bool)>,
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
    mut fields: Query<(
        &mut GrassField,
        &mut Transform,
        &GrassOf,
        Option<&mut GrassEdit>,
    )>,
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
        let had_floor = floor.herbs.is_some() || floor.litter.is_some() || floor.ferns.is_some();
        // A new field takes the grass field's heights when the build didn't bring them.
        let grass_heights = || {
            linked
                .and_then(|g| fields.get(g.0).ok())
                .map(|(f, ..)| f.heights.clone())
        };
        let known_heights = built.heights.clone().or_else(grass_heights);
        let known_floor = built.floor_heights.clone().or_else(grass_heights);
        let mut wanted = [false; 3];
        for (layer, style, density, dryness) in built.layers {
            let slot = match layer {
                FloorLayer::Grass => None,
                FloorLayer::Herbs => Some(0),
                FloorLayer::Litter => Some(1),
                FloorLayer::Ferns => Some(2),
            };
            if let Some(i) = slot {
                wanted[i] = true;
            }
            let existing = match slot {
                None => linked.map(|g| g.0),
                Some(0) => floor.herbs,
                Some(1) => floor.litter,
                Some(_) => floor.ferns,
            };
            match existing.and_then(|e| fields.get_mut(e).ok()) {
                Some((mut field, mut t, _, edit)) => {
                    let heights = match slot {
                        None => &built.heights,
                        Some(_) => &built.floor_heights,
                    };
                    let (kinds, styles) = layer_kinds(layer, &built.floor_kinds);
                    // A rebuild that changed nothing leaves the field alone (its GPU maps and
                    // ground cover stay as they are); one that changed a little says where.
                    let reshaped = field.size != built.size
                        || field.resolution != built.resolution
                        || field.style != style
                        || field.styles != styles;
                    let new_heights = heights.as_ref().filter(|h| **h != field.heights);
                    let exact = field.density == density
                        && field.dryness == dryness
                        && field.kinds == kinds;
                    if reshaped || !exact || new_heights.is_some() {
                        if let Some(mut edit) = edit {
                            if reshaped {
                                // Repainted whole: the baseline starts again from these maps.
                                edit.painted = CoverBaseline::default();
                            }
                            let points = changed_points(
                                built.resolution,
                                &mut edit.painted,
                                (&density, &dryness, &kinds),
                            );
                            edit.all = reshaped || points.is_none();
                            edit.points = points.unwrap_or_default();
                        }
                        if let Some(h) = new_heights {
                            field.heights = h.clone();
                        }
                        field.size = built.size;
                        field.resolution = built.resolution;
                        field.density = density;
                        field.dryness = dryness;
                        field.style = style;
                        field.kinds = kinds;
                        field.styles = styles;
                    }
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
                    let (kinds, styles) = layer_kinds(layer, &built.floor_kinds);
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
                        kinds,
                        styles,
                    };
                    let e = commands
                        .spawn((
                            field_at,
                            GrassEdit::new_field(&field),
                            field,
                            GrassOf(region_e),
                            layer,
                        ))
                        .id();
                    match slot {
                        None => {
                            commands.entity(region_e).insert(RegionGrass(e));
                        }
                        Some(0) => floor.herbs = Some(e),
                        Some(1) => floor.litter = Some(e),
                        Some(_) => floor.ferns = Some(e),
                    }
                }
            }
        }
        // Layers the region no longer draws.
        for (i, slot) in [&mut floor.herbs, &mut floor.litter, &mut floor.ferns]
            .into_iter()
            .enumerate()
        {
            if !wanted[i]
                && let Some(e) = slot.take()
            {
                commands.entity(e).despawn();
            }
        }
        if floor.herbs.is_some() || floor.litter.is_some() || floor.ferns.is_some() || had_floor {
            commands.entity(region_e).insert(floor);
        }
    }

    due.0 |= terrain.read().count() > 0;
    // The forest floor's surface built or rebuilt: the floor layers stand on it.
    due.0 |= surfaces.iter().any(|(s, _)| s.is_changed());
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
    // A new season or settings restyle every region; one still building then picks it up
    // when it is done (not at the next timed round, which a frozen clock never reaches).
    due.1 |= settings.is_changed() || season.is_changed();
    if *timer > 0.0 && !fresh && !due.0 && !due.1 {
        return;
    }
    *timer = REBUILD_SECS;
    // Fields whose region is gone.
    for (e, of) in &orphans {
        if !regions.contains(of.0) {
            commands.entity(e).despawn();
        }
    }
    let relief = std::mem::take(&mut due.0);
    let restyle = std::mem::take(&mut due.1);
    let season_dry = grass_dryness(season.day_of_year);
    // Bracken browns earlier and further than the meadow.
    let fern_dry = season_dry.max(crate::season::bracken_brown(season.day_of_year));
    let day = season.day_of_year;
    for (region_e, region, state, linked, floor, build) in &regions {
        let styles = if settings.gpu_grass {
            let mut styles = floor_styles(&library, region);
            styles.retain(|(l, _)| settings.gpu_ferns || *l != FloorLayer::Ferns);
            for (l, style) in &mut styles {
                if *l == FloorLayer::Ferns {
                    winter_ferns(style, day);
                }
            }
            styles
        } else {
            Vec::new()
        };
        if styles.is_empty() {
            if let Some(g) = linked {
                commands.entity(g.0).despawn();
                commands.entity(region_e).remove::<RegionGrass>();
            }
            if let Some(f) = floor {
                for e in [f.herbs, f.litter, f.ferns].into_iter().flatten() {
                    commands.entity(e).despawn();
                }
                commands.entity(region_e).remove::<RegionFloor>();
            }
            continue;
        }
        if build.is_some() {
            // Still building: the next round picks up what changed meanwhile.
            due.0 |= relief;
            due.1 |= restyle;
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
        let ferns: Vec<_> = plants
            .iter()
            .filter(|(p, _, r)| r.0 == Some(region_e) && p.stage.is_alive())
            .filter(|(p, _, _)| library.get(p.species).form == PlantForm::Fern)
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
            relief || existing.is_none_or(|(f, ..)| f.heights.is_empty() || f.resolution != res);
        let shades = shades_floor(&styles);
        let herb_kinds_on = settings.herb_kinds;
        let ferns_layer = styles.iter().any(|(l, _)| *l == FloorLayer::Ferns);
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
            let (mut density, mut dryness) =
                grass_maps(&region, &soil, center, res, &grass, &covers);
            // Fern crowns grow as their own floor layer: fronds drawn on the GPU, not a mesh
            // and an entity each (`visuals` draws none for ferns where a region has it).
            let mut ferns = ferns_layer.then(|| fern_maps(&region, res, &ferns, &covers, fern_dry));
            // Autumn browns the meadow and winter leaves it straw, wherever it grows.
            for d in &mut dryness {
                *d += (1.0 - *d) * season_dry;
            }
            let mut floor = shades.then(|| {
                let mut covers = covers;
                covers.extend(trunks);
                floor_maps(
                    &region, &soil, center, res, &density, &crowns, &covers, &grids, season_dry,
                    day,
                )
            });
            // Heath shrubs crowd the grass out where they carpet the ground.
            if let Some(f) = &floor {
                for (d, s) in density.iter_mut().zip(&f.shrubs) {
                    *d *= 1.0 - 0.75 * s;
                }
            }
            let kinds = grass_kinds(&region, &soil, center, res, &crowns);
            // The meadow's kinds come from its summer look; winter then mats and bleaches
            // them all, and the meadow's own blades with them.
            let mut kind_styles = styles
                .iter()
                .find(|(l, _)| *l == FloorLayer::Grass)
                .map_or_else(Vec::new, |(_, s)| crate::meadow::palette(s, day));
            for s in &mut kind_styles {
                winter_grass(s, day);
            }
            let herb_styles = herb_styles_on(day).1;
            let heights = ground.map(|g| heights(g.as_ref(), center, &region, res));
            let floor_heights = heights
                .as_ref()
                .filter(|_| shades)
                .map(|h| onto_floor(h, &grids, center, &region, res));
            let mut maps = Some((density, dryness));
            let herb_kinds = floor
                .as_mut()
                .filter(|_| herb_kinds_on)
                .map_or_else(Vec::new, |f| std::mem::take(&mut f.herb_kinds));
            let litter_kinds = floor
                .as_mut()
                .filter(|_| herb_kinds_on)
                .map_or_else(Vec::new, |f| std::mem::take(&mut f.litter_kinds));
            let mut floor = floor.map(|f| [Some(f.herbs), Some(f.litter)]);
            let layers = styles
                .into_iter()
                .filter_map(|(layer, mut style)| {
                    if layer == FloorLayer::Grass {
                        winter_grass(&mut style, day);
                    }
                    let (d, w) = match layer {
                        FloorLayer::Grass => maps.take()?,
                        FloorLayer::Herbs => floor.as_mut()?[0].take()?,
                        FloorLayer::Litter => floor.as_mut()?[1].take()?,
                        FloorLayer::Ferns => ferns.take()?,
                    };
                    Some((layer, style, d, w))
                })
                .collect();
            BuiltGrass {
                layers,
                floor_kinds: FloorKinds {
                    kinds,
                    kind_styles,
                    herb_kinds,
                    herb_styles,
                    litter_kinds,
                },
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

/// A layer's kinds map and the other kinds' looks: the meadow's grasses, the floor's herbs.
fn layer_kinds(layer: FloorLayer, built: &FloorKinds) -> (Vec<u8>, Vec<GrassStyle>) {
    match layer {
        FloorLayer::Grass => (built.kinds.clone(), built.kind_styles.clone()),
        FloorLayer::Herbs if !built.herb_kinds.is_empty() => {
            (built.herb_kinds.clone(), built.herb_styles.clone())
        }
        FloorLayer::Litter if !built.litter_kinds.is_empty() => {
            (built.litter_kinds.clone(), vec![GrassStyle::twigs()])
        }
        _ => (Vec::new(), Vec::new()),
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

    #[test]
    fn seasons_redden_then_strip_bilberry_mat_the_meadow_and_brown_heather() {
        let (july, january) = (190.0, 20.0);
        let mut summer = GrassStyle::default();
        winter_grass(&mut summer, july);
        assert_eq!(
            summer,
            GrassStyle {
                dead: thatch(july),
                ..GrassStyle::default()
            },
            "summer meadows stand as they are, a sixth of their blades dead"
        );
        // Last year's straw: most in spring before the new growth overtops it, least at
        // midsummer, more again as the grass dies back.
        assert!(thatch(110.0) > 0.3 && (0.12..0.2).contains(&thatch(july)));
        assert!(thatch(300.0) > thatch(july) + 0.15);
        let mut winter = GrassStyle::default();
        winter_grass(&mut winter, january);
        assert!(winter.lying > 0.5 && winter.height < summer.height);
        // Weathered buff: less yellow than summer's straw (blue closer to red).
        let warmth = |c: [f32; 3]| c[2] / c[0];
        assert!(warmth(winter.dry_color) > warmth(summer.dry_color) + 0.1);

        let b = DWARF_SHRUB_KIND as usize - 1;
        let (_, jul) = herb_styles_on(july);
        let (_, jan) = herb_styles_on(january);
        assert!(
            jul[b].alternate_leaves > 0.0 && jul[b].width > 0.05 && jul[b].bloom > 0.03,
            "bilberry in leaf and berry in July"
        );
        assert!(
            jan[b].alternate_leaves == 0.0 && jan[b].width < 0.01 && jan[b].bloom == 0.0,
            "bare twigs in January"
        );
        assert!(
            jan[b].dry_color[1] > jan[b].dry_color[0],
            "the twigs stay green"
        );
        let (_, oct) = herb_styles_on(285.0);
        let c = oct[b].lush_color;
        assert!(
            c[0] > 3.0 * c[1] && oct[b].alternate_leaves > 0.0,
            "crimson leaves in October"
        );
        assert!(crate::season::bracken_brown(285.0) > 0.6);
        assert_eq!(
            crate::season::bracken_brown(190.0),
            0.0,
            "bracken green in July"
        );
        let (_, apr) = herb_styles_on(110.0);
        assert!(apr[2].bloom > 0.4, "anemone carpets white in April");
        assert!(
            jul[2].height < 0.3 * apr[2].height,
            "and is gone to ground by July"
        );
        assert!(
            jul[0].bloom < 0.1 && apr[0].bloom > 0.15,
            "sorrel flowers in spring"
        );
        assert!(anemone_season(110.0) > 2.0 && anemone_season(190.0) < 0.2);
        assert_eq!(anemone_season(150.0), 1.0);
        assert!(
            jan[b + 1].bloom > 0.02
                && oct[b + 1].bloom > jan[b + 1].bloom
                && jul[b + 1].bloom == 0.0,
            "lingonberries red from August, some through winter"
        );
        let (_, aug) = herb_styles_on(225.0);
        let mauve = |c: [f32; 3]| c[2] / c[0];
        assert!(
            mauve(aug[b + 2].bloom_color) > 0.8,
            "heather in flower in August"
        );
        assert!(
            mauve(jan[b + 2].bloom_color) < 0.4,
            "dead russet spikes in January"
        );
    }

    #[test]
    fn heath_shrubs_take_acid_ground_by_its_water_and_light() {
        let best = |w: [f32; 3]| (0..3).max_by(|&a, &b| w[a].total_cmp(&w[b])).unwrap();
        // Moist floor under spruce with some light: bilberry.
        let spruce = dwarf_shrubs(0.7, 0.7, 0.3, 0.6);
        assert_eq!(best(spruce), 0);
        assert!(spruce[0] > 0.5);
        // Dry floor under pines: lingonberry.
        assert_eq!(best(dwarf_shrubs(0.6, 0.6, 0.15, 0.6)), 1);
        // Open dry ridge: heather.
        let ridge = dwarf_shrubs(0.1, 0.1, 0.12, 0.3);
        assert_eq!(best(ridge), 2);
        assert!(ridge[2] > 0.5);
        // Under broadleaves on rich moist soil, none of them.
        assert!(dwarf_shrubs(0.9, 0.0, 0.4, 0.0).iter().all(|w| *w < 0.05));
        // Their kinds follow the herbs'.
        let (_, kinds) = herb_styles();
        assert_eq!(kinds.len(), DWARF_SHRUB_KIND as usize + 3);
        assert_eq!(kinds[DWARF_SHRUB_KIND as usize - 1], GrassStyle::bilberry());
        assert_eq!(kinds[BRAMBLE_KIND as usize - 1], GrassStyle::bramble());
    }

    #[test]
    fn bramble_takes_light_broadleaf_shade_on_fresh_soil() {
        // Under a thin oak canopy on fresh, fairly rich soil: thickets.
        let oaks = bramble_want(0.5, 0.5, 0.3, 0.5, 0.0);
        assert!(oaks > 0.6, "{oaks}");
        // Under spruce, on needles: none.
        assert!(bramble_want(0.8, 0.0, 0.3, 0.5, 0.8) < 0.05);
        // Deep shade: weak.
        assert!(bramble_want(1.6, 1.6, 0.3, 0.5, 0.0) < 0.4 * oaks);
        // A clearing in the broadleaf wood: thickets too.
        assert!(bramble_want(0.0, 0.0, 0.3, 0.5, 0.0) > 0.6);
        // Waterlogged: none.
        assert!(bramble_want(0.5, 0.5, 0.85, 0.5, 0.0) < 0.05);
        // In flower at midsummer, black berries by September, nothing in winter.
        let mid = crate::season::bramble(crate::season::MIDSUMMER);
        assert!(mid[0] > 0.9 && mid[1] < 0.1, "{mid:?}");
        let sept = crate::season::bramble(245.0);
        assert!(sept[0] < 0.05 && sept[1] > 0.9 && sept[2] > 0.9, "{sept:?}");
        assert!(sept[3] < 0.05 && crate::season::bramble(320.0)[3] > 0.9);
        assert!(
            crate::season::bramble(20.0)
                .iter()
                .take(2)
                .all(|v| *v < 0.01)
        );
        let (_, kinds) = herb_styles_on(crate::season::MIDSUMMER);
        assert!(kinds[BRAMBLE_KIND as usize - 1].bloom > 0.1);
    }

    #[test]
    fn a_trails_shoulder_is_beside_its_tread_only() {
        let region = EcologyRegion {
            corridors: vec![[0.0, 0.0, 1.9]],
            keep_clear: vec![[0.0, 0.0, 2.2], [0.0, 4.0, 2.0]],
            ..EcologyRegion::default()
        };
        let at = |x: f32, z: f32| trail_shoulder(&region, Vec2::new(x, z));
        // The tread itself, then its shoulder, then open ground.
        assert!(!at(1.0, 0.0));
        assert!(at(-2.0, 0.0));
        assert!(!at(3.0, 0.0));
        // Inside the pond's own keep-clear circle, beside the trail: not a shoulder.
        assert!(!at(0.0, 2.4));
    }

    #[test]
    fn each_herb_takes_its_own_woods() {
        let calm = [0.0; 4];
        // Spruce over moist, needle-strewn ground: wood sorrel.
        assert_eq!(herb_kind(0.9, 0.0, 0.5, 0.8, calm, 1.0), 1);
        // A beech hall on rich moist soil: woodruff.
        assert_eq!(herb_kind(0.9, 1.0, 0.5, 0.0, calm, 1.0), 2);
        // A light, dry oak wood: wood anemone.
        assert_eq!(herb_kind(0.4, 1.0, 0.15, 0.0, calm, 1.0), 3);
        // Dry shade under conifers: the broad shade herbs.
        assert_eq!(herb_kind(0.7, 0.0, 0.1, 0.0, calm, 1.0), 0);
        // A colony of another herb can push into a wood that suits it less.
        assert_eq!(
            herb_kind(0.9, 1.0, 0.5, 0.0, [-0.5, -0.8, -0.8, 0.9], 1.0),
            3
        );
    }
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
        assert_eq!(layers.len(), 4, "{layers:?}");
        // The forest's ferns grow as fronds in a layer of their own, where its fern plants are.
        let (f, l, _) = q
            .get(&app.world, floor.ferns.expect("a fern layer"))
            .unwrap();
        assert_eq!(*l, FloorLayer::Ferns);
        let on = f.density.iter().filter(|d| **d > 0.0).count() as f32;
        let share = on / f.density.len() as f32;
        assert!((0.005..0.8).contains(&share), "ferns cover {share}");
        assert_eq!(f.heights.len(), f.density.len());
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

    /// The ground cover is painted once and then only where the grass changes: not at all
    /// while the forest stands still (its maps are rebuilt every few seconds all the same), and
    /// only the tiles its growth changed while it grows (it used to repaint whole fields every
    /// rebuild, 650 cells a frame here).
    #[test]
    fn the_ground_cover_is_repainted_only_where_the_grass_changed() {
        let mut app = app();
        let region = app
            .world
            .spawn((Transform::default(), crate::demo::forest_region()))
            .id();
        // Until the forest has grown in (its history runs as a catch-up, a step at a time).
        let start = std::time::Instant::now();
        loop {
            app.update_n(30);
            let state = app.world.get::<RegionState>(region);
            let grown = state
                .is_some_and(|s| s.catch_up.is_none() && !s.stepping() && s.pending_years == 0.0);
            if grown || start.elapsed().as_secs() > 120 {
                break;
            }
        }
        settle(&mut app);
        let painted = |app: &App| {
            app.world
                .resource::<crate::cover::CoverPainted>()
                .cells_painted
        };
        // Two rebuild rounds of real time; cells painted a frame.
        let run = |app: &mut App| {
            let (cells, frames) = (painted(app), app.world.resource::<Time>().frame_count());
            let start = std::time::Instant::now();
            while start.elapsed().as_secs_f32() < 2.5 * REBUILD_SECS {
                app.update();
                std::thread::sleep(std::time::Duration::from_millis(4));
            }
            settle(app);
            let frames = app.world.resource::<Time>().frame_count() - frames;
            (painted(app) - cells) as f32 / frames.max(1) as f32
        };
        app.world.resource_mut::<EcologySettings>().paused = true;
        while app
            .world
            .get::<RegionState>(region)
            .is_some_and(|s| s.stepping())
        {
            app.update();
        }
        // A floor layer may still take the forest floor's surface as it lands; then nothing.
        let still = (0..4).map(|_| run(&mut app)).find(|c| *c == 0.0);
        app.world.resource_mut::<EcologySettings>().paused = false;
        let growing = run(&mut app);
        println!("ground cover: {still:?} cells a frame still, {growing:.0} growing");
        assert_eq!(still, Some(0.0), "a still forest kept repainting its cover");
        assert!(growing < 150.0, "{growing} cells a frame");
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
                // Late May: wood anemone in leaf, done flowering (weighs as it always did).
                150.0,
            )
        };
        let FloorMaps {
            herbs: (herbs, _),
            litter: (litter, _),
            ..
        } = maps(0.0);
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
        let herbs_in_grass = maps(1.0).herbs.0;
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
