//! Far woods: cheap 3D woods from the edge of the drawn trees out to a few kilometers, where
//! the terrain says the land is wooded ([`engine_world::ForestCover`]). Seen from a hill or a
//! low flight, a wood 0.5–5 km off is a mass of crowns with ridgeline silhouettes, which a
//! texture painted on the ground can't give.
//!
//! The land around the camera is cut into square cells; each cell is one mesh per tree
//! species of crossed crown cards (and a flat card on top for crowns seen from above),
//! sampling the species' mass atlas ([`crate::assets::SpeciesVisual::leaves_far`]). Cards are
//! spaced wider with distance (each far one stands for a few crowns), so a closed wood costs
//! about the same per pixel at any distance. Cells stream in nearest first, a few a frame, and
//! rebuild when the camera has moved far enough that they want another spacing. Ground that
//! already holds [`SceneryTree`]s is left to them, so a valley's own woods and the far woods
//! around it meet without doubling up.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use engine_assets::{Assets, Handle};
use engine_core::prelude::*;
use engine_render::{Camera, DitherFade, Mesh, MeshRenderer, NoVirtualGeometry, StandardMaterial};
use engine_world::{ForestCover, Ground, WorldShift};

use crate::assets::FoliageVisuals;
use crate::meshgen::MeshBuilder;
use crate::meshgen::textures::{CLUSTER_CELL, LeafKind, atlas_cell};
use crate::near_woods::NearWoodsTree;
use crate::scenery_tree::SceneryTree;
use crate::species::PlantLibrary;
use crate::visuals::camera_position;

/// Where and how far woods are drawn.
#[derive(Resource, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FarWoods {
    pub enabled: bool,
    /// Far woods start this far from the camera, meters (inside it: the real trees).
    pub start_m: f32,
    /// And end this far, meters (beyond it: the terrain's painted woods).
    pub end_m: f32,
    /// Side of a streamed cell, meters.
    pub cell_m: f32,
    /// Card spacing per meter of distance (0.01: a card every 10 m at a kilometer), never
    /// closer than `min_spacing_m`.
    pub spacing_per_m: f32,
    pub min_spacing_m: f32,
    /// Worker threads building cells (0: build up to `cells_per_frame` cells in the frame).
    pub background_builds: u32,
    /// Most cells built per frame when `background_builds` is 0.
    pub cells_per_frame: u32,
    /// More places far woods leave alone: no far woods within `radius` meters of `center`
    /// (world X/Z). Ground under [`SceneryTree`]s needs none: it is kept clear on its own.
    pub keep_clear: Vec<KeepClear>,
}

/// A circle (world X/Z) far woods leave alone.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct KeepClear {
    pub center: DVec3,
    pub radius: f32,
}

impl Default for FarWoods {
    fn default() -> Self {
        Self {
            enabled: true,
            start_m: 500.0,
            end_m: 5000.0,
            cell_m: 256.0,
            spacing_per_m: 0.01,
            min_spacing_m: 6.0,
            background_builds: 2,
            cells_per_frame: 6,
            keep_clear: Vec::new(),
        }
    }
}

/// The trees far woods are made of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum FarTree {
    Spruce,
    Pine,
    Oak,
    Birch,
}

impl FarTree {
    const ALL: [FarTree; 4] = [FarTree::Spruce, FarTree::Pine, FarTree::Oak, FarTree::Birch];

    pub(crate) fn species(self) -> &'static str {
        match self {
            FarTree::Spruce => "spruce",
            FarTree::Pine => "pine",
            FarTree::Oak => "oak",
            FarTree::Birch => "birch",
        }
    }

    fn kind(self) -> LeafKind {
        match self {
            FarTree::Spruce => LeafKind::ShortNeedles,
            FarTree::Pine => LeafKind::LongNeedles,
            FarTree::Oak => LeafKind::Lobed,
            FarTree::Birch => LeafKind::Small,
        }
    }

    /// Height range (m), and the crown: where it starts (share of the height) and how wide
    /// it is (share of the height).
    fn shape(self) -> ((f32, f32), f32, f32) {
        match self {
            FarTree::Spruce => ((18.0, 30.0), 0.12, 0.36),
            FarTree::Pine => ((17.0, 26.0), 0.55, 0.42),
            FarTree::Oak => ((15.0, 24.0), 0.3, 0.8),
            FarTree::Birch => ((14.0, 22.0), 0.35, 0.45),
        }
    }
}

/// One card of a far wood: a tree (or, far off, a few) at `foot` (cell-local).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct FarCard {
    pub tree: FarTree,
    pub foot: Vec3,
    pub height: f32,
    /// Crown width (m): at least the spacing, so a closed wood stays closed.
    pub width: f32,
    pub yaw: f32,
    pub shade: f32,
}

/// Cover lattice step, meters: closer card spacings read the cover from a lattice this fine
/// (bilinear), so a cell asks the terrain a few hundred times rather than once per tree.
const COVER_STEP_M: f32 = 16.0;

/// The cards of one cell: candidates every `spacing` meters (jittered), each kept with the
/// wood's density there and a conifer by its conifer share. `cover` answers for a world X/Z
/// (`(density, conifer)`); `ground` gives the ground height there (asked only for kept
/// cards). Cards `keep_out` says are left out.
pub(crate) fn cell_cards(
    origin: DVec3,
    cell_m: f32,
    spacing: f32,
    seed: u64,
    cover: &dyn Fn(DVec3) -> Option<(f32, f32)>,
    ground: &dyn Fn(DVec3) -> Option<f64>,
    keep_out: &dyn Fn(DVec3) -> bool,
) -> Vec<FarCard> {
    let at = |x: f32, z: f32| origin + DVec3::new(x as f64, 0.0, z as f64);
    // Most of the land is open: a coarse look first, so an open cell costs a few samples.
    const PROBES: i64 = 5;
    let wooded = (0..PROBES * PROBES).any(|k| {
        let f = |c: i64| (c as f32 + 0.5) / PROBES as f32 * cell_m;
        cover(at(f(k % PROBES), f(k / PROBES))).is_some()
    });
    if !wooded {
        return Vec::new();
    }
    // Density and conifer share (weighted by density) at a cell-local X/Z.
    let lattice: Option<(usize, Vec<(f32, f32)>)> = (spacing < COVER_STEP_M).then(|| {
        let n = (cell_m / COVER_STEP_M).ceil() as usize + 1;
        let values = (0..n * n)
            .map(|k| {
                let c = cover(at(
                    (k % n) as f32 * COVER_STEP_M,
                    (k / n) as f32 * COVER_STEP_M,
                ));
                c.map_or((0.0, 0.0), |(d, c)| (d, c * d))
            })
            .collect();
        (n, values)
    });
    let woods_at = |x: f32, z: f32| -> (f32, f32) {
        match &lattice {
            Some((n, v)) => {
                let (fx, fz) = (x / COVER_STEP_M, z / COVER_STEP_M);
                let (ix, iz) = ((fx as usize).min(n - 2), (fz as usize).min(n - 2));
                let (tx, tz) = (fx - ix as f32, fz - iz as f32);
                let g = |i: usize, j: usize| v[j * n + i];
                let lerp = |a: (f32, f32), b: (f32, f32), t: f32| {
                    (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
                };
                let (d, c) = lerp(
                    lerp(g(ix, iz), g(ix + 1, iz), tx),
                    lerp(g(ix, iz + 1), g(ix + 1, iz + 1), tx),
                    tz,
                );
                (d, if d > 0.0 { c / d } else { 0.0 })
            }
            None => cover(at(x, z)).unwrap_or((0.0, 0.0)),
        }
    };
    let n = (cell_m / spacing).ceil().max(1.0) as i64;
    let mut cards = Vec::new();
    for j in 0..n {
        for i in 0..n {
            let h = |k: i64| engine_core::math::hash01(i, j, k, seed);
            let x = (i as f32 + h(0)) * spacing;
            let z = (j as f32 + h(1)) * spacing;
            if x >= cell_m || z >= cell_m {
                continue;
            }
            let (density, conifer) = woods_at(x, z);
            if h(2) >= density.clamp(0.0, 1.0) {
                continue;
            }
            let flat = at(x, z);
            if keep_out(flat) {
                continue;
            }
            let Some(y) = ground(flat) else {
                continue;
            };
            let pick = h(3);
            let tree = if h(4) < conifer.clamp(0.0, 1.0) {
                if pick < 0.6 {
                    FarTree::Spruce
                } else {
                    FarTree::Pine
                }
            } else if pick < 0.7 {
                FarTree::Oak
            } else {
                FarTree::Birch
            };
            let ((lo, hi), _, wide) = tree.shape();
            let height = lo + (hi - lo) * h(5);
            cards.push(FarCard {
                tree,
                foot: Vec3::new(x, (y - origin.y) as f32, z),
                height,
                width: (height * wide).max(spacing * 1.4),
                yaw: h(6) * std::f32::consts::PI,
                shade: 0.62 + 0.22 * h(7),
            });
        }
    }
    cards
}

/// The meshes of a cell's cards, one per tree kind that has any.
pub(crate) fn cell_meshes(cards: &[FarCard]) -> Vec<(FarTree, Mesh)> {
    let mut builders: HashMap<FarTree, MeshBuilder> = HashMap::new();
    for c in cards {
        let b = builders.entry(c.tree).or_default();
        let ((_, _), base, _) = c.tree.shape();
        let kind = c.tree.kind();
        let conifer = matches!(c.tree, FarTree::Spruce | FarTree::Pine);
        // Conifers stand as the tall spray cells, broad crowns as the round cluster.
        let cell = if conifer {
            if c.shade > 0.73 { 1 } else { 0 }
        } else {
            CLUSTER_CELL
        };
        let (uv0, uv1) = atlas_cell(kind, cell);
        // A card standing for a clump of crowns (far out, where cards are wider than one
        // crown) keeps a crown's proportions: its band grows with its width, or it would read
        // as a flat disc hanging over the slope.
        let half = c.width * 0.5;
        let y1 = c.foot.y + c.height;
        // Wide cards also sink a little: far terrain is drawn coarser than the ground it was
        // sampled from, lower on ridges, where a card standing on the true height would float.
        let band = (c.height * (1.0 - base)).max(c.width * 0.55);
        let sink = c.width * 0.15;
        let y0 = (y1 - band).max(c.foot.y - 1.0) - sink;
        let y1 = y0 + band;
        let lit = |up: bool| {
            let v = if up { c.shade } else { c.shade * 0.55 };
            Vec4::new(v, v, v, 1.0)
        };
        for k in 0..2 {
            let a = c.yaw + k as f32 * std::f32::consts::FRAC_PI_2;
            let u = Vec3::new(a.cos(), 0.0, a.sin());
            let corners = [
                c.foot.with_y(y0) - u * half,
                c.foot.with_y(y0) + u * half,
                c.foot.with_y(y1) + u * half,
                c.foot.with_y(y1) - u * half,
            ];
            let uvs = [
                Vec2::new(uv0.x, uv1.y),
                Vec2::new(uv1.x, uv1.y),
                Vec2::new(uv1.x, uv0.y),
                Vec2::new(uv0.x, uv0.y),
            ];
            // Shade like a rounded crown: each side turns outward and up.
            let first = b.vertices.len() as u32;
            for (i, (&p, &uv)) in corners.iter().zip(&uvs).enumerate() {
                let side = if i == 0 || i == 3 { -u } else { u };
                let n = (side + Vec3::Y * if i >= 2 { 0.9 } else { 0.3 }).normalize();
                b.vertex(p, n, uv, lit(i >= 2));
            }
            b.tri(first, first + 1, first + 2);
            b.tri(first, first + 2, first + 3);
            b.tri(first, first + 2, first + 1);
            b.tri(first, first + 3, first + 2);
        }
        {
            // The crown seen from above: a low pyramid from partway up the band to its top,
            // which closes the canopy from a hill and from the side stays inside the crown's
            // outline (a flat card there shows as a disc floating over the slope). Broad
            // crowns reach a little wider than their upright cards, over the gaps their round
            // outlines leave; a spruce's spire is a narrow one.
            let (uv0, uv1) = atlas_cell(kind, CLUSTER_CELL);
            let (from, reach) = if matches!(c.tree, FarTree::Spruce) {
                (0.35, 0.6)
            } else {
                (0.5, 1.25)
            };
            let y = y0 + (y1 - y0) * from;
            let half = half * reach;
            let (s, co) = c.yaw.sin_cos();
            let (u, w) = (Vec3::new(co, 0.0, s), Vec3::new(-s, 0.0, co));
            let rim = [
                c.foot.with_y(y) - u * half - w * half,
                c.foot.with_y(y) + u * half - w * half,
                c.foot.with_y(y) + u * half + w * half,
                c.foot.with_y(y) - u * half + w * half,
            ];
            let apex = c.foot.with_y(y1);
            let mid = (uv0.x + uv1.x) * 0.5;
            for i in 0..4 {
                let (p, q) = (rim[i], rim[(i + 1) % 4]);
                let out = ((p + q) * 0.5 - c.foot.with_y(y)).normalize_or(Vec3::X);
                let n = (out * 0.6 + Vec3::Y).normalize();
                let first = b.vertices.len() as u32;
                b.vertex(p, n, Vec2::new(uv0.x, uv1.y), lit(true));
                b.vertex(q, n, Vec2::new(uv1.x, uv1.y), lit(true));
                b.vertex(apex, n, Vec2::new(mid, uv0.y), lit(true));
                b.tri(first, first + 1, first + 2);
                b.tri(first, first + 2, first + 1);
            }
        }
    }
    let mut out: Vec<_> = builders.into_iter().map(|(t, b)| (t, b.finish())).collect();
    out.sort_by_key(|(t, _)| FarTree::ALL.iter().position(|a| a == t));
    out
}

/// A streamed cell: its grid key, the spacing it was built with, and its drawn parts.
#[derive(Component, Debug, Clone)]
pub struct FarWoodCell {
    band: i32,
    /// Where the camera was when it was built (for cells near the inner edge).
    built_from: DVec3,
    /// Its meshes, dropped with it.
    meshes: Vec<Handle<Mesh>>,
    /// The cover's version it was built from (see [`FarWoodsState::epoch`]).
    epoch: u64,
}

/// A cell coming in: its cards dither in, then the cell it replaces (`ghost`) dithers out on
/// the opposite pattern, over [`CELL_FADE_S`] ([`crate::lod_fade::overlapped`]).
#[derive(Component, Debug, Clone, Copy)]
pub struct CellFadeIn {
    t: f32,
    ghost: Option<Entity>,
}

/// A cell on its way out: no longer streamed, drawn on while it dithers away (by itself, or
/// timed by the [`CellFadeIn`] of the cell replacing it, `by`), then dropped.
#[derive(Component, Debug, Clone, Copy)]
pub struct CellGhost {
    t: f32,
    by: Option<Entity>,
}

/// How long a far cell takes to dither in or out, seconds. Longer than a near part's fade: a
/// whole hillside of crowns changing at once is seen, a single tree's level barely.
pub const CELL_FADE_S: f32 = 1.0;

/// Streaming state: the grid's origin (moved by world shifts), the cells by key and the
/// cells being built.
#[derive(Resource, Default)]
pub struct FarWoodsState {
    pub(crate) origin: DVec3,
    cells: HashMap<(i64, i64), Entity>,
    pending: HashMap<(i64, i64), Pending>,
    /// Bumped whenever the terrain installs a new cover (a terrain edit, a terrain added or
    /// moved). Cells from older ones rebuild once the cover has held still for
    /// [`COVER_SETTLE_FRAMES`]: a terrain that reinstalls its cover every frame (one that
    /// moves) doesn't keep every wood rebuilding or blinking out.
    pub(crate) epoch: u64,
    pub(crate) quiet_frames: u32,
    /// Still materials (no sway) per tree kind, made from the species' far leaves.
    materials: HashMap<FarTree, Handle<StandardMaterial>>,
    /// The ground scenery trees stand on, and whether it must be looked at again.
    pub(crate) occupied: Arc<Occupied>,
    occupied_stale: bool,
}

/// Ground holding placed scenery trees (not the near woods' own): the [`OCCUPIED_M`] squares (keyed
/// from the grid's origin, so world shifts carry it along) within [`OCCUPIED_REACH_M`] of one.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Occupied(HashSet<(i64, i64)>);

/// Side of an occupied square, meters.
const OCCUPIED_M: f64 = 16.0;
/// How far around a scenery tree its ground counts as taken, meters (about a crown's reach,
/// so a wood's edge trees keep far cards out of their crowns).
const OCCUPIED_REACH_M: f64 = 10.0;

impl Occupied {
    fn key(origin: DVec3, p: DVec3) -> (i64, i64) {
        (
            ((p.x - origin.x) / OCCUPIED_M).floor() as i64,
            ((p.z - origin.z) / OCCUPIED_M).floor() as i64,
        )
    }

    fn from_trees(origin: DVec3, trees: impl Iterator<Item = DVec3>) -> Self {
        let mut set = HashSet::new();
        let r = DVec3::new(OCCUPIED_REACH_M, 0.0, OCCUPIED_REACH_M);
        for p in trees {
            let (a, b) = (Self::key(origin, p - r), Self::key(origin, p + r));
            for j in a.1..=b.1 {
                for i in a.0..=b.0 {
                    set.insert((i, j));
                }
            }
        }
        Self(set)
    }

    pub(crate) fn holds(&self, origin: DVec3, p: DVec3) -> bool {
        self.0.contains(&Self::key(origin, p))
    }
}

/// A cell being built on a worker thread.
struct Pending {
    epoch: u64,
    band: i32,
    built_from: DVec3,
    /// Where the cell's corner is (moved by world shifts while it builds).
    origin: DVec3,
    build: Build,
}

/// A cell's build: on a worker thread, or done in the frame.
enum Build {
    Running(std::thread::JoinHandle<Vec<(FarTree, Mesh)>>),
    Ready(Vec<(FarTree, Mesh)>),
}

impl Build {
    fn is_done(&self) -> bool {
        match self {
            Build::Running(h) => h.is_finished(),
            Build::Ready(_) => true,
        }
    }

    /// The finished meshes (`None` if the worker panicked).
    fn take(self) -> Option<Vec<(FarTree, Mesh)>> {
        match self {
            Build::Running(h) => h.join().ok(),
            Build::Ready(parts) => Some(parts),
        }
    }
}

/// An inner cell built for a camera further than this from where it is now is dropped at once
/// (its cards could stand among the real trees); within it, it draws on until rebuilt.
const JUMPED_M: f32 = 160.0;

/// Everything a cell's build needs, owned, so it can run on a worker thread.
struct CellJob {
    origin: DVec3,
    cell_m: f32,
    spacing: f32,
    seed: u64,
    cam: DVec3,
    start_m: f32,
    keep_clear: Vec<KeepClear>,
    /// The ground scenery trees hold, keyed from `grid_origin`.
    occupied: Arc<Occupied>,
    grid_origin: DVec3,
    cover: Arc<dyn engine_world::ForestQuery>,
    ground: Option<Ground>,
}

impl CellJob {
    fn run(self) -> Vec<(FarTree, Mesh)> {
        let flat_distance = |p: DVec3| DVec2::new(p.x - self.cam.x, p.z - self.cam.z).length();
        let keep_out = |p: DVec3| {
            flat_distance(p) < self.start_m as f64
                || self.occupied.holds(self.grid_origin, p)
                || self.keep_clear.iter().any(|k| {
                    DVec2::new(p.x - k.center.x, p.z - k.center.z).length() < k.radius as f64
                })
        };
        let ground_y = |p: DVec3| match &self.ground {
            Some(g) => g.height(DVec3::new(p.x, self.cam.y + 10_000.0, p.z)),
            None => Some(0.0),
        };
        let cover_at = |p: DVec3| self.cover.sample(p).map(|s| (s.density, s.conifer));
        let cards = cell_cards(
            self.origin,
            self.cell_m,
            self.spacing,
            self.seed,
            &cover_at,
            &ground_y,
            &keep_out,
        );
        cell_meshes(&cards)
    }
}

/// Frames the cover must hold still before cells built from an older one rebuild.
pub(crate) const COVER_SETTLE_FRAMES: u32 = 60;

/// Spacing band for a distance: spacing doubles every two bands.
fn band(settings: &FarWoods, d: f32) -> i32 {
    let s = (d * settings.spacing_per_m).max(settings.min_spacing_m);
    ((s / settings.min_spacing_m).log2() * 2.0).floor() as i32
}

fn band_spacing(settings: &FarWoods, band: i32) -> f32 {
    settings.min_spacing_m * 2f32.powf(band as f32 * 0.5)
}

/// Despawn a cell and free its meshes.
fn drop_cell(
    e: Entity,
    cells: &Query<&FarWoodCell>,
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
) {
    if let Ok(c) = cells.get(e) {
        for &h in &c.meshes {
            meshes.remove(h);
        }
    }
    commands.entity(e).try_despawn();
}

/// Take a cell out of the stream: dithering away by itself when `smooth`, else at once. A cell
/// still dithering in over another lets that one go at once.
fn retire(
    e: Entity,
    smooth: bool,
    cells: &Query<&FarWoodCell>,
    incoming: &Query<&CellFadeIn>,
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
) {
    if let Ok(&CellFadeIn { ghost: Some(g), .. }) = incoming.get(e) {
        drop_cell(g, cells, commands, meshes);
    }
    if smooth {
        commands
            .entity(e)
            .try_remove::<CellFadeIn>()
            .try_insert(CellGhost { t: 0.0, by: None });
    } else {
        drop_cell(e, cells, commands, meshes);
    }
}

/// Move the cells' fades on: dither the cards of cells coming in and going, and drop the gone.
#[allow(clippy::type_complexity)]
pub fn fade_far_cells(
    mut commands: Commands,
    time: Res<Time>,
    mut incoming: Query<(Entity, &mut CellFadeIn, Option<&Children>)>,
    mut ghosts: Query<(Entity, &mut CellGhost, Option<&Children>), Without<CellFadeIn>>,
    mut dithers: Query<&mut DitherFade>,
    cells: Query<&FarWoodCell>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let step = time.delta_secs() / CELL_FADE_S;
    let mut set = |commands: &mut Commands, children: Option<&Children>, d: Option<DitherFade>| {
        for &c in children.into_iter().flatten() {
            match (dithers.get_mut(c), d) {
                (Ok(mut now), Some(d)) => {
                    if *now != d {
                        *now = d;
                    }
                }
                (Err(_), Some(d)) => {
                    commands.entity(c).try_insert(d);
                }
                (Ok(_), None) => {
                    commands.entity(c).try_remove::<DitherFade>();
                }
                (Err(_), None) => {}
            }
        }
    };
    for (e, mut fade, children) in &mut incoming {
        fade.t = (fade.t + step).min(1.0);
        let (out, into) = crate::lod_fade::overlapped(fade.t);
        let ghost = fade
            .ghost
            .and_then(|g| ghosts.get(g).ok().map(|(_, _, c)| c));
        if fade.t >= 1.0 {
            set(&mut commands, children, None);
            if let Some(g) = fade.ghost {
                drop_cell(g, &cells, &mut commands, &mut meshes);
            }
            commands.entity(e).try_remove::<CellFadeIn>();
        } else {
            set(&mut commands, children, Some(into));
            if let Some(children) = ghost {
                set(&mut commands, children, Some(out));
            }
        }
    }
    for (e, mut ghost, children) in &mut ghosts {
        match ghost.by {
            Some(by) if incoming.contains(by) => continue,
            // The cell replacing it went first (despawned by someone else): it goes alone.
            Some(_) => ghost.by = None,
            None => {}
        }
        ghost.t = (ghost.t + step).min(1.0);
        if ghost.t >= 1.0 {
            drop_cell(e, &cells, &mut commands, &mut meshes);
        } else {
            set(&mut commands, children, Some(DitherFade(ghost.t)));
        }
    }
}

/// Stream the cells around the camera: drop those out of range, build missing ones and
/// rebuild those that want another spacing, nearest first: on `background_builds` worker
/// threads (the terrain's answers cost tens of microseconds each), or a few a frame in the
/// frame itself when that is 0.
#[allow(clippy::too_many_arguments)]
pub fn stream_far_woods(
    mut commands: Commands,
    settings: Res<FarWoods>,
    library: Res<PlantLibrary>,
    visuals: Res<FoliageVisuals>,
    cover: Res<ForestCover>,
    ground: Option<Res<Ground>>,
    cams: Query<(&Camera, &GlobalTransform)>,
    mut state: ResMut<FarWoodsState>,
    cells: Query<&FarWoodCell>,
    trees: Query<Ref<GlobalTransform>, (With<SceneryTree>, Without<NearWoodsTree>)>,
    mut removed: RemovedComponents<SceneryTree>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    (fades, incoming, ghosts): (
        Res<crate::lod_fade::LodFades>,
        Query<&CellFadeIn>,
        Query<Entity, With<CellGhost>>,
    ),
) {
    let smooth = fades.settled();
    // Scenery trees planted, felled or moved: look at their ground again (it changes the
    // woods like a new cover does).
    if removed.read().count() > 0 || trees.iter().any(|t| t.is_changed()) {
        state.occupied_stale = true;
    }
    // A tree spawned this frame still stands at the origin; wait for its world transform.
    let trees_placed = !trees.iter().any(|t| t.is_added());
    let mut occupied_changed = false;
    if state.occupied_stale && trees_placed {
        state.occupied_stale = false;
        let occupied = Occupied::from_trees(state.origin, trees.iter().map(|t| t.translation()));
        if occupied != *state.occupied {
            state.occupied = Arc::new(occupied);
            occupied_changed = true;
        }
    }
    if cover.is_changed() || occupied_changed {
        state.epoch += 1;
        state.quiet_frames = 0;
    } else {
        state.quiet_frames = state.quiet_frames.saturating_add(1);
    }
    let settled = state.quiet_frames >= COVER_SETTLE_FRAMES;
    if !settings.enabled || settings.is_changed() {
        for (_, e) in std::mem::take(&mut state.cells) {
            drop_cell(e, &cells, &mut commands, &mut meshes);
        }
        for e in &ghosts {
            drop_cell(e, &cells, &mut commands, &mut meshes);
        }
        // Builds still running finish on their own; their results are dropped.
        state.pending.clear();
        if !settings.enabled {
            return;
        }
    }
    let Some(cam) = camera_position(&cams) else {
        return;
    };
    if !trees_placed {
        return;
    }
    // The trees' still far materials, once their visuals exist.
    if state.materials.len() < FarTree::ALL.len() {
        for t in FarTree::ALL {
            if state.materials.contains_key(&t) {
                continue;
            }
            let Some(vis) = library.id(t.species()).and_then(|id| visuals.get(id)) else {
                continue;
            };
            let Some(mut m) = materials.get(vis.leaves_far[0]).cloned() else {
                continue;
            };
            m.wind_sway = 0.0;
            let h = materials.add(m);
            state.materials.insert(t, h);
        }
        if state.materials.is_empty() {
            return;
        }
    }
    let cell_m = settings.cell_m.max(16.0);
    let origin = state.origin;
    let reach = settings.end_m + cell_m;
    let corner = |key: (i64, i64)| {
        origin
            + DVec3::new(
                key.0 as f64 * cell_m as f64,
                0.0,
                key.1 as f64 * cell_m as f64,
            )
    };
    let center =
        |key: (i64, i64)| corner(key) + DVec3::new(cell_m as f64 * 0.5, 0.0, cell_m as f64 * 0.5);
    let flat_distance = |p: DVec3| DVec2::new(p.x - cam.x, p.z - cam.z).length() as f32;
    // Drop cells out of range.
    let gone: Vec<_> = state
        .cells
        .keys()
        .copied()
        .filter(|&k| flat_distance(center(k)) > reach)
        .collect();
    for k in gone {
        if let Some(e) = state.cells.remove(&k) {
            retire(e, smooth, &cells, &incoming, &mut commands, &mut meshes);
        }
    }
    // What each cell in range wants, nearest first.
    let (ci, cj) = (
        ((cam.x - origin.x) / cell_m as f64).floor() as i64,
        ((cam.z - origin.z) / cell_m as f64).floor() as i64,
    );
    let r = (reach / cell_m).ceil() as i64;
    let half_diag = cell_m * std::f32::consts::FRAC_1_SQRT_2;
    let mut todo = Vec::new();
    for j in cj - r..=cj + r {
        for i in ci - r..=ci + r {
            let d = flat_distance(center((i, j)));
            if d + half_diag < settings.start_m {
                // Wholly inside the start (the camera jumped or ran into it): the real trees
                // stand here, so its cards go rather than drawing over them, even while a
                // rebuild of it is still running (that build is dropped when it lands).
                if let Some(e) = state.cells.remove(&(i, j)) {
                    retire(e, smooth, &cells, &incoming, &mut commands, &mut meshes);
                }
                continue;
            }
            if state.pending.contains_key(&(i, j)) {
                continue;
            }
            if d > settings.end_m + half_diag {
                continue;
            }
            let b = band(&settings, d.max(settings.start_m));
            let inner = d - half_diag < settings.start_m + 64.0;
            let stale = match state.cells.get(&(i, j)).and_then(|&e| cells.get(e).ok()) {
                None => true,
                Some(c) => {
                    c.band != b
                        || (inner && flat_distance(c.built_from) > 64.0)
                        || (settled && c.epoch != state.epoch)
                }
            };
            if stale {
                // Built for a camera far from here, an inner cell's cards may stand right
                // where the real trees are now: they go at once, not when the rebuild lands.
                let moved_far = state
                    .cells
                    .get(&(i, j))
                    .and_then(|&e| cells.get(e).ok())
                    .is_some_and(|c| inner && flat_distance(c.built_from) > JUMPED_M);
                if moved_far && let Some(e) = state.cells.remove(&(i, j)) {
                    retire(e, false, &cells, &incoming, &mut commands, &mut meshes);
                }
                todo.push((d, (i, j), b));
            }
        }
    }
    todo.sort_by(|a, b| a.0.total_cmp(&b.0));
    let epoch = state.epoch;
    let threads = settings.background_builds as usize;
    let room = if threads > 0 {
        threads.saturating_sub(state.pending.len())
    } else {
        settings.cells_per_frame as usize
    };
    for (_, key, b) in todo.into_iter().take(room) {
        let job = CellJob {
            origin: corner(key),
            cell_m,
            spacing: band_spacing(&settings, b),
            seed: (key.0 as u64).wrapping_mul(0x9e37_79b9)
                ^ (key.1 as u64).wrapping_mul(0x85eb_ca6b),
            cam,
            start_m: settings.start_m,
            keep_clear: settings.keep_clear.clone(),
            occupied: state.occupied.clone(),
            grid_origin: origin,
            cover: cover.0.clone(),
            ground: ground.as_deref().cloned(),
        };
        let build = if threads > 0 {
            Build::Running(std::thread::spawn(move || job.run()))
        } else {
            Build::Ready(job.run())
        };
        state.pending.insert(
            key,
            Pending {
                epoch,
                band: b,
                built_from: cam,
                origin: corner(key),
                build,
            },
        );
    }
    // Place the cells whose builds are done.
    let done: Vec<_> = state
        .pending
        .iter()
        .filter(|(_, p)| p.build.is_done())
        .map(|(&k, _)| k)
        .collect();
    for key in done {
        let p = state.pending.remove(&key).expect("pending cell");
        let Some(parts) = p.build.take() else {
            continue;
        };
        // The camera has come in over it while it built.
        if flat_distance(center(key)) + half_diag < settings.start_m {
            continue;
        }
        let e = commands.spawn(Transform::from_translation(p.origin)).id();
        // The cell it replaces dithers out under it; with nothing to replace it comes in alone.
        let ghost = match state.cells.remove(&key) {
            Some(old) if smooth => {
                if let Ok(&CellFadeIn { ghost: Some(g), .. }) = incoming.get(old) {
                    drop_cell(g, &cells, &mut commands, &mut meshes);
                }
                commands
                    .entity(old)
                    .try_remove::<CellFadeIn>()
                    .try_insert(CellGhost {
                        t: 0.0,
                        by: Some(e),
                    });
                Some(old)
            }
            Some(old) => {
                retire(old, false, &cells, &incoming, &mut commands, &mut meshes);
                None
            }
            None => None,
        };
        let mut handles = Vec::new();
        for (tree, mesh) in parts {
            let Some(&material) = state.materials.get(&tree) else {
                continue;
            };
            let mut r = MeshRenderer::new(
                engine_render::Shape::Cuboid { size: [0.01; 3] },
                StandardMaterial::default(),
            );
            let h = meshes.add(mesh);
            handles.push(h);
            r.mesh = engine_render::MeshRef::Handle(h);
            r.material = engine_render::MaterialRef::Handle(material);
            r.cast_shadows = false;
            let card = commands
                .spawn((Transform::default(), ChildOf(e), r, NoVirtualGeometry))
                .id();
            if smooth {
                commands.entity(card).insert(DitherFade(-1.0));
            }
        }
        if smooth {
            commands.entity(e).insert(CellFadeIn { t: 0.0, ghost });
        }
        commands.entity(e).insert(FarWoodCell {
            band: p.band,
            built_from: p.built_from,
            meshes: handles,
            epoch: p.epoch,
        });
        state.cells.insert(key, e);
    }
}

/// Follow a world shift: a translation moves the grid with the cells; a turn rebuilds them.
pub fn follow_world_shift(
    mut commands: Commands,
    mut shifts: MessageReader<WorldShift>,
    mut state: ResMut<FarWoodsState>,
    cells: Query<&FarWoodCell>,
    ghosts: Query<Entity, With<CellGhost>>,
    mut meshes: Option<ResMut<Assets<Mesh>>>,
) {
    for s in shifts.read() {
        if s.is_translation_only() {
            state.origin = s.point(state.origin);
            for p in state.pending.values_mut() {
                p.origin = s.point(p.origin);
                p.built_from = s.point(p.built_from);
            }
        } else {
            state.occupied_stale = true;
            state.pending.clear();
            let cells_now = std::mem::take(&mut state.cells).into_values();
            for e in cells_now.chain(ghosts.iter()) {
                if let (Ok(c), Some(meshes)) = (cells.get(e), meshes.as_mut()) {
                    for &h in &c.meshes {
                        meshes.remove(h);
                    }
                }
                commands.entity(e).try_despawn();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn flat(_: DVec3) -> Option<f64> {
        Some(0.0)
    }

    #[test]
    fn cards_follow_the_cover_and_close_the_canopy() {
        let o = DVec3::new(1000.0, 0.0, 0.0);
        let full = |_: DVec3| Some((1.0, 1.0));
        let cards = cell_cards(o, 128.0, 8.0, 3, &full, &flat, &|_| false);
        // A card on every candidate: a closed wood.
        assert!(cards.len() > 220 && cards.len() <= 256, "{}", cards.len());
        assert!(
            cards
                .iter()
                .all(|c| matches!(c.tree, FarTree::Spruce | FarTree::Pine))
        );
        assert!(cards.iter().all(|c| c.width >= 8.0 * 1.15));
        assert!(cards.iter().all(|c| (0.0..128.0).contains(&c.foot.x)));
        let broad = |_: DVec3| Some((0.5, 0.0));
        let half = cell_cards(o, 128.0, 8.0, 3, &broad, &flat, &|_| false);
        let share = half.len() as f32 / cards.len() as f32;
        assert!((0.35..0.65).contains(&share), "{share}");
        assert!(
            half.iter()
                .all(|c| matches!(c.tree, FarTree::Oak | FarTree::Birch))
        );
        // Kept-clear places get none.
        let west = cell_cards(o, 128.0, 8.0, 3, &full, &flat, &|p| p.x < 1064.0);
        assert!(!west.is_empty() && west.iter().all(|c| c.foot.x >= 64.0));
        // Each card is two crossed cards (four triangles each, both sides) and a four-sided
        // cap (two triangles a side).
        let meshes = cell_meshes(&cards);
        let tris: usize = meshes.iter().map(|(_, m)| m.triangle_count()).sum();
        assert_eq!(tris, cards.len() * 16);
    }

    #[test]
    fn a_clump_card_keeps_a_crowns_proportions() {
        // Far out a card stands for a clump: 60 m wide, on 20 m trees.
        let card = FarCard {
            tree: FarTree::Oak,
            foot: Vec3::new(0.0, 5.0, 0.0),
            height: 20.0,
            width: 60.0,
            yaw: 0.3,
            shade: 0.8,
        };
        let meshes = cell_meshes(&[card]);
        let ys = meshes[0].1.vertices.iter().map(|v| v.position[1]);
        let (lo, hi) = ys.fold((f32::MAX, f32::MIN), |(a, b), y| (a.min(y), b.max(y)));
        // Not a 6 m band under a 60 m disc: a mound at least half as tall as it is wide, its
        // foot a little into the ground.
        assert!(hi - lo >= 30.0, "{lo}..{hi}");
        assert!((5.0 - 60.0 * 0.2..5.0).contains(&lo), "{lo}");
    }

    #[test]
    fn open_land_costs_a_few_samples() {
        let asked = Cell::new(0);
        let none = |_: DVec3| {
            asked.set(asked.get() + 1);
            None
        };
        let cards = cell_cards(DVec3::ZERO, 256.0, 6.0, 1, &none, &flat, &|_| false);
        assert!(cards.is_empty());
        assert!(asked.get() <= 25, "{}", asked.get());
    }

    struct Everywhere;
    impl engine_world::ForestQuery for Everywhere {
        fn sample(&self, _p: DVec3) -> Option<engine_world::ForestSample> {
            Some(engine_world::ForestSample {
                density: 0.6,
                conifer: 1.0,
            })
        }
    }

    /// An app with conifer far woods 100–600 m around a camera at the origin, built in the
    /// frame.
    fn woods_app() -> (App, Entity) {
        let mut app = App::new();
        app.world
            .resource_mut::<Time>()
            .set_source(engine_core::time::TimeSource::Manual {
                step_secs: 1.0 / 60.0,
            });
        app.add_plugins(engine_world::WorldPlugin);
        app.init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>()
            .init_resource::<Assets<engine_render::Image>>()
            .add_plugins(crate::FoliagePlugin);
        app.world.insert_resource(crate::FoliageLoading::in_frame());
        // Conifers only (building every species' visuals would take a while).
        let all = PlantLibrary::default();
        let keep = ["pine", "spruce"].map(|n| all.get(all.id(n).unwrap()).clone());
        app.world.insert_resource(PlantLibrary {
            species: keep.to_vec(),
        });
        app.world.insert_resource(FarWoods {
            start_m: 100.0,
            end_m: 600.0,
            cell_m: 128.0,
            background_builds: 0,
            cells_per_frame: u32::MAX,
            ..Default::default()
        });
        let cam = app
            .world
            .spawn((Camera::default(), Transform::from_translation(DVec3::ZERO)))
            .id();
        (app, cam)
    }

    #[test]
    fn cells_stream_around_the_camera_and_free_their_meshes() {
        let (mut app, cam) = woods_app();
        // No woods painted: nothing drawn.
        app.update_n(3);
        let mut cells = app.world.query::<(&FarWoodCell, &Transform)>();
        assert_eq!(
            cells
                .iter(&app.world)
                .filter(|(c, _)| !c.meshes.is_empty())
                .count(),
            0
        );
        app.world
            .insert_resource(ForestCover(std::sync::Arc::new(Everywhere)));
        // A terrain reinstalling its cover every frame (a moving one) doesn't keep the woods
        // rebuilding: the open cells stay until it holds still.
        for _ in 0..10 {
            app.world.resource_mut::<ForestCover>().set_changed();
            app.update();
        }
        assert_eq!(
            cells
                .iter(&app.world)
                .filter(|(c, _)| !c.meshes.is_empty())
                .count(),
            0
        );
        app.update_n(COVER_SETTLE_FRAMES as usize + 2);
        let drawn: Vec<_> = cells
            .iter(&app.world)
            .filter(|(c, _)| !c.meshes.is_empty())
            .map(|(_, t)| t.translation)
            .collect();
        assert!(drawn.len() > 40, "{}", drawn.len());
        // A ring: none further than the end, none wholly inside the start.
        for p in &drawn {
            let d = DVec2::new(p.x + 64.0, p.z + 64.0).length();
            assert!(d < 600.0 + 91.0 && d > 100.0 - 91.0, "{p}");
        }
        let meshes = app.world.resource::<Assets<Mesh>>().len();
        // The camera flies 3 km: the old cells go, with their meshes.
        app.world.get_mut::<Transform>(cam).unwrap().translation = DVec3::new(3000.0, 0.0, 0.0);
        app.update_n(2);
        assert!(
            cells
                .iter(&app.world)
                .all(|(_, t)| t.translation.x > 3000.0 - 700.0)
        );
        let after = app.world.resource::<Assets<Mesh>>().len();
        assert!(after <= meshes + meshes / 10, "{after} vs {meshes}");
        // On worker threads (the default): the same cells arrive over a few frames.
        app.world.resource_mut::<FarWoods>().background_builds = 2;
        let mut frames = 0;
        while cells
            .iter(&app.world)
            .filter(|(c, _)| !c.meshes.is_empty())
            .count()
            < drawn.len() / 2
        {
            app.update();
            std::thread::sleep(std::time::Duration::from_millis(2));
            frames += 1;
            assert!(frames < 5000, "worker builds never arrived");
        }
    }

    #[test]
    fn a_camera_jumping_into_the_woods_leaves_no_cards_round_it() {
        let (mut app, cam) = woods_app();
        // A start wide enough for cells to lie wholly inside it.
        app.world.resource_mut::<FarWoods>().start_m = 300.0;
        app.world
            .insert_resource(ForestCover(std::sync::Arc::new(Everywhere)));
        app.update_n(COVER_SETTLE_FRAMES as usize + 2);
        // A jump of 500 m (a tour or walk stop, a sprint over a few seconds): cells that stood
        // in the ring are now wholly inside the start, where the real trees are.
        let to = DVec3::new(500.0, 0.0, 0.0);
        app.world.get_mut::<Transform>(cam).unwrap().translation = to;
        app.update_n(2);
        let mut cells = app.world.query::<(&FarWoodCell, &Transform)>();
        for (c, t) in cells.iter(&app.world) {
            if c.meshes.is_empty() {
                continue;
            }
            let center = t.translation + DVec3::new(64.0, 0.0, 64.0);
            let d = DVec2::new(center.x - to.x, center.z - to.z).length();
            assert!(
                d > 300.0 - 91.0,
                "far woods cell left at {d:.0} m from the camera"
            );
        }
    }

    #[test]
    fn cells_cross_fade_as_the_camera_flies_over() {
        let (mut app, cam) = woods_app();
        app.world
            .insert_resource(ForestCover(std::sync::Arc::new(Everywhere)));
        app.update_n(COVER_SETTLE_FRAMES as usize + 2);
        let meshes = app.world.resource::<Assets<Mesh>>().len();
        // A low flight at 600 m/s (10 m a frame): cells come into the ring, leave it and are
        // rebuilt with other spacings on the way.
        let (mut crossed, mut alone) = (0, 0);
        for f in 1..=60 {
            app.world.get_mut::<Transform>(cam).unwrap().translation =
                DVec3::new(10.0 * f as f64, 0.0, 0.0);
            app.update();
            let mut incoming = app.world.query::<&CellFadeIn>();
            crossed += incoming
                .iter(&app.world)
                .filter(|c| c.ghost.is_some())
                .count();
            let mut ghosts = app.world.query::<&CellGhost>();
            alone += ghosts.iter(&app.world).filter(|g| g.by.is_none()).count();
        }
        assert!(crossed > 0, "no rebuilt cell cross-faded");
        assert!(alone > 0, "no cell faded out by itself");
        // Something else despawns cells mid-fade (an editor's Stop clearing the scene): the
        // woods carry on without them rather than failing on their commands.
        let some: Vec<Entity> = app
            .world
            .query_filtered::<Entity, Or<(With<CellGhost>, With<CellFadeIn>)>>()
            .iter(&app.world)
            .take(6)
            .collect();
        assert!(!some.is_empty());
        for e in some {
            app.world.despawn(e);
        }
        for f in 61..=70 {
            app.world.get_mut::<Transform>(cam).unwrap().translation =
                DVec3::new(10.0 * f as f64, 0.0, 0.0);
            app.update();
        }
        // A second and a half later every fade is over: no ghosts, nothing dithered, and the
        // gone cells' meshes freed.
        app.update_n(90);
        assert_eq!(app.world.query::<&CellGhost>().iter(&app.world).count(), 0);
        assert_eq!(app.world.query::<&CellFadeIn>().iter(&app.world).count(), 0);
        assert_eq!(app.world.query::<&DitherFade>().iter(&app.world).count(), 0);
        let after = app.world.resource::<Assets<Mesh>>().len();
        assert!(after <= meshes + meshes / 4, "{after} vs {meshes}");
        // A cut (a tour stop, a teleport) swaps at once.
        app.world.get_mut::<Transform>(cam).unwrap().translation = DVec3::new(3000.0, 0.0, 0.0);
        app.update_n(3);
        assert_eq!(app.world.query::<&CellGhost>().iter(&app.world).count(), 0);
        assert_eq!(app.world.query::<&CellFadeIn>().iter(&app.world).count(), 0);
    }

    /// Triangles drawn by every far woods cell (not counting those fading out).
    fn woods_triangles(app: &mut App) -> usize {
        let mut cells = app
            .world
            .query_filtered::<&FarWoodCell, Without<CellGhost>>();
        let handles: Vec<_> = cells
            .iter(&app.world)
            .flat_map(|c| c.meshes.clone())
            .collect();
        let meshes = app.world.resource::<Assets<Mesh>>();
        handles
            .iter()
            .filter_map(|&h| meshes.get(h))
            .map(|m| m.triangle_count())
            .sum()
    }

    #[test]
    fn scenery_trees_keep_their_own_ground() {
        let (mut app, _) = woods_app();
        app.world
            .insert_resource(ForestCover(std::sync::Arc::new(Everywhere)));
        app.update_n(3);
        let open = woods_triangles(&mut app);
        assert!(open > 0);
        // A 200 m square wood of scenery trees 300 m out: the far woods make room for it.
        let library = app.world.resource::<PlantLibrary>().clone();
        let mut wood = Vec::new();
        for j in 0..25 {
            for i in 0..25 {
                let p = DVec3::new(200.0 + i as f64 * 8.0, 0.0, -100.0 + j as f64 * 8.0);
                let tree = SceneryTree::new(&library, "pine", (i * 31 + j) as u64).unwrap();
                wood.push(app.world.spawn((Transform::from_translation(p), tree)).id());
            }
        }
        app.update_n(COVER_SETTLE_FRAMES as usize + 4);
        let around = woods_triangles(&mut app);
        // The ring holds about 1.1 km² of woods; the square takes 0.04 km² of it.
        let share = 1.0 - around as f64 / open as f64;
        assert!((0.015..0.08).contains(&share), "{share}");
        // Felled: the far woods fill the gap again.
        for e in wood {
            app.world.despawn(e);
        }
        app.update_n(COVER_SETTLE_FRAMES as usize + 4);
        assert_eq!(woods_triangles(&mut app), open);
    }

    #[test]
    fn occupied_ground_reaches_a_crown_around_each_tree() {
        let o = DVec3::new(5.0, 0.0, -3.0);
        let occ = Occupied::from_trees(o, [DVec3::new(100.0, 7.0, 40.0)].into_iter());
        assert!(occ.holds(o, DVec3::new(100.0, 0.0, 40.0)));
        assert!(occ.holds(o, DVec3::new(100.0 + OCCUPIED_REACH_M - 0.5, 0.0, 40.0)));
        assert!(!occ.holds(o, DVec3::new(100.0 + OCCUPIED_M * 2.0 + 1.0, 0.0, 40.0)));
        assert!(occ.0.len() <= 4);
    }

    #[test]
    fn spacing_widens_with_distance() {
        let s = FarWoods::default();
        assert_eq!(band_spacing(&s, band(&s, 500.0)), 6.0);
        let far = band_spacing(&s, band(&s, 4000.0));
        assert!((28.0..=40.0).contains(&far), "{far}");
        assert!(band(&s, 1000.0) < band(&s, 2000.0));
    }
}
