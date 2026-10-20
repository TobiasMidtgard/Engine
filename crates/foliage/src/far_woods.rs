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
//! rebuild when the camera has moved far enough that they want another spacing.

use std::collections::HashMap;

use engine_assets::{Assets, Handle};
use engine_core::prelude::*;
use engine_render::{Camera, Mesh, MeshRenderer, NoVirtualGeometry, StandardMaterial};
use engine_world::{ForestCover, Ground, WorldShift};

use crate::assets::FoliageVisuals;
use crate::meshgen::MeshBuilder;
use crate::meshgen::textures::{CLUSTER_CELL, LeafKind, atlas_cell};
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
    /// Most cells built per frame.
    pub cells_per_frame: u32,
    /// Places with trees of their own (a valley of scenery trees): no far woods within
    /// `radius` meters of `center` (world X/Z).
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

    fn species(self) -> &'static str {
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

/// The cards of one cell: candidates every `spacing` meters (jittered), each kept with the
/// wood's density there and a conifer by its conifer share. `cover` answers for a world
/// position on the ground; `ground` gives the ground height at a world X/Z. Cards within
/// `keep_out` (a world X/Z and radius) are left out.
pub(crate) fn cell_cards(
    origin: DVec3,
    cell_m: f32,
    spacing: f32,
    seed: u64,
    cover: &dyn Fn(DVec3) -> Option<(f32, f32)>,
    ground: &dyn Fn(DVec3) -> Option<f64>,
    keep_out: &dyn Fn(DVec3) -> bool,
) -> Vec<FarCard> {
    // Most of the land is open: a coarse look first, so an open cell costs a few samples.
    const PROBES: i64 = 5;
    let wooded = (0..PROBES * PROBES).any(|k| {
        let f = |c: i64| (c as f64 + 0.5) / PROBES as f64 * cell_m as f64;
        let flat = origin + DVec3::new(f(k % PROBES), 0.0, f(k / PROBES));
        ground(flat).is_some_and(|y| cover(DVec3::new(flat.x, y, flat.z)).is_some())
    });
    if !wooded {
        return Vec::new();
    }
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
            let flat = origin + DVec3::new(x as f64, 0.0, z as f64);
            if keep_out(flat) {
                continue;
            }
            let Some(y) = ground(flat) else {
                continue;
            };
            let at = DVec3::new(flat.x, y, flat.z);
            let Some((density, conifer)) = cover(at) else {
                continue;
            };
            if h(2) >= density.clamp(0.0, 1.0) {
                continue;
            }
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
                foot: (at - origin).as_vec3(),
                height,
                width: (height * wide).max(spacing * 1.15),
                yaw: h(6) * std::f32::consts::PI,
                shade: 0.85 + 0.25 * h(7),
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
            if c.shade > 0.97 { 1 } else { 0 }
        } else {
            CLUSTER_CELL
        };
        let (uv0, uv1) = atlas_cell(kind, cell);
        let y0 = c.foot.y + c.height * base;
        let y1 = c.foot.y + c.height;
        let half = c.width * 0.5;
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
        if !matches!(c.tree, FarTree::Spruce) {
            // The crown seen from above.
            let (uv0, uv1) = atlas_cell(kind, CLUSTER_CELL);
            let y = y0 + (y1 - y0) * 0.72;
            let (s, co) = c.yaw.sin_cos();
            let (u, w) = (Vec3::new(co, 0.0, s), Vec3::new(-s, 0.0, co));
            let corners = [
                c.foot.with_y(y) - u * half - w * half,
                c.foot.with_y(y) + u * half - w * half,
                c.foot.with_y(y) + u * half + w * half,
                c.foot.with_y(y) - u * half + w * half,
            ];
            let uvs = [
                Vec2::new(uv0.x, uv1.y),
                Vec2::new(uv1.x, uv1.y),
                Vec2::new(uv1.x, uv0.y),
                Vec2::new(uv0.x, uv0.y),
            ];
            b.card(corners, uvs, Vec3::Y, [lit(true); 4]);
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
}

/// Streaming state: the grid's origin (moved by world shifts) and the cells by key.
#[derive(Resource, Default)]
pub struct FarWoodsState {
    origin: DVec3,
    cells: HashMap<(i64, i64), Entity>,
    /// Still materials (no sway) per tree kind, made from the species' far leaves.
    materials: HashMap<FarTree, Handle<StandardMaterial>>,
}

/// Spacing band for a distance: spacing doubles every two bands.
fn band(settings: &FarWoods, d: f32) -> i32 {
    let s = (d * settings.spacing_per_m).max(settings.min_spacing_m);
    ((s / settings.min_spacing_m).log2() * 2.0).floor() as i32
}

fn band_spacing(settings: &FarWoods, band: i32) -> f32 {
    settings.min_spacing_m * 2f32.powf(band as f32 * 0.5)
}

/// Stream the cells around the camera: drop those out of range, build missing ones and
/// rebuild those that want another spacing, nearest first, a few a frame.
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
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let drop_cell = |e: Entity, commands: &mut Commands, meshes: &mut Assets<Mesh>| {
        if let Ok(c) = cells.get(e) {
            for &h in &c.meshes {
                meshes.remove(h);
            }
        }
        commands.entity(e).despawn();
    };
    if !settings.enabled || settings.is_changed() || cover.is_changed() {
        for (_, e) in std::mem::take(&mut state.cells) {
            drop_cell(e, &mut commands, &mut meshes);
        }
        if !settings.enabled {
            return;
        }
    }
    let Some(cam) = camera_position(&cams) else {
        return;
    };
    // The trees' still far materials, once their visuals exist.
    if state.materials.len() < FarTree::ALL.len() {
        for t in FarTree::ALL {
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
    let center = |key: (i64, i64)| {
        origin
            + DVec3::new(
                (key.0 as f64 + 0.5) * cell_m as f64,
                0.0,
                (key.1 as f64 + 0.5) * cell_m as f64,
            )
    };
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
            drop_cell(e, &mut commands, &mut meshes);
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
            if d > settings.end_m + half_diag || d + half_diag < settings.start_m {
                continue;
            }
            let b = band(&settings, d.max(settings.start_m));
            let inner = d - half_diag < settings.start_m + 64.0;
            let stale = match state.cells.get(&(i, j)).and_then(|&e| cells.get(e).ok()) {
                None => true,
                Some(c) => c.band != b || (inner && flat_distance(c.built_from) > 64.0),
            };
            if stale {
                todo.push((d, (i, j), b));
            }
        }
    }
    todo.sort_by(|a, b| a.0.total_cmp(&b.0));
    let keep_out = |p: DVec3| {
        flat_distance(p) < settings.start_m
            || settings
                .keep_clear
                .iter()
                .any(|k| DVec2::new(p.x - k.center.x, p.z - k.center.z).length() < k.radius as f64)
    };
    let ground_y = |p: DVec3| match &ground {
        Some(g) => g.height(DVec3::new(p.x, cam.y + 10_000.0, p.z)),
        None => Some(0.0),
    };
    let cover_at = |p: DVec3| cover.sample(p).map(|s| (s.density, s.conifer));
    for (_, key, b) in todo.into_iter().take(settings.cells_per_frame as usize) {
        let cell_origin = origin
            + DVec3::new(
                key.0 as f64 * cell_m as f64,
                0.0,
                key.1 as f64 * cell_m as f64,
            );
        let seed =
            (key.0 as u64).wrapping_mul(0x9e37_79b9) ^ (key.1 as u64).wrapping_mul(0x85eb_ca6b);
        let cards = cell_cards(
            cell_origin,
            cell_m,
            band_spacing(&settings, b),
            seed,
            &cover_at,
            &ground_y,
            &keep_out,
        );
        if let Some(e) = state.cells.remove(&key) {
            drop_cell(e, &mut commands, &mut meshes);
        }
        let e = commands
            .spawn(Transform::from_translation(cell_origin))
            .id();
        let mut handles = Vec::new();
        for (tree, mesh) in cell_meshes(&cards) {
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
            commands.spawn((Transform::default(), ChildOf(e), r, NoVirtualGeometry));
        }
        commands.entity(e).insert(FarWoodCell {
            band: b,
            built_from: cam,
            meshes: handles,
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
    mut meshes: Option<ResMut<Assets<Mesh>>>,
) {
    for s in shifts.read() {
        if s.is_translation_only() {
            state.origin = s.point(state.origin);
        } else {
            for (_, e) in std::mem::take(&mut state.cells) {
                if let (Ok(c), Some(meshes)) = (cells.get(e), meshes.as_mut()) {
                    for &h in &c.meshes {
                        meshes.remove(h);
                    }
                }
                commands.entity(e).despawn();
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
        // Each card is two crossed cards (and a top for all but spruce), four triangles each.
        let meshes = cell_meshes(&cards);
        let tris: usize = meshes.iter().map(|(_, m)| m.triangle_count()).sum();
        assert!(
            tris >= cards.len() * 8 && tris <= cards.len() * 12,
            "{tris}"
        );
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

    #[test]
    fn cells_stream_around_the_camera_and_free_their_meshes() {
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
            cells_per_frame: u32::MAX,
            ..Default::default()
        });
        let cam = app
            .world
            .spawn((Camera::default(), Transform::from_translation(DVec3::ZERO)))
            .id();
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
        app.update_n(2);
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
