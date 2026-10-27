//! Rock fields: the stones and boulders lying on airless and desert worlds, as landers and
//! rovers see them (Apollo and Curiosity photos are full of them; a bare, smooth plain reads
//! as fake from the ground).
//!
//! Rocks lie in cells: quadtree patches of the terrain's own tiling (so the scatter is the same
//! wherever the world is anchored), each one entity with one mesh of every rock in it, in the
//! cell's tangent frame. Two tiers: stones (12–60 cm) in small cells near every focus point,
//! boulders (60 cm up to [`RockField::max_size_m`]) in larger cells farther out. Sizes follow
//! the power law of rock counts on the Moon and Mars (Golombek & Rapp 1997: the number of rocks
//! wider than D falls as a power of D), and rocky ground comes in patches a few hundred
//! meters across, rockier on slopes. Rocks are placed from a cell's hash, so the same rocks
//! come back when you return. Each rock is one of a few fractured-stone shapes (a lumpy
//! sphere cut by planes into facets), turned, tilted to the ground and sunk into it.
//!
//! Cells take a few hundred full terrain samples, so they are built on worker threads a few
//! at a time and appear when done.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use std::thread::JoinHandle;

use engine_assets::{Assets, Handle};
use engine_core::math::{DQuat, DVec3};
use engine_core::prelude::*;
use engine_render::{
    MaterialPreset, MaterialRef, Mesh, MeshRef, MeshRenderer, NoMeshLod, NoVirtualGeometry,
    StandardMaterial, Vertex,
};
use engine_world::FocusPoints;
use serde::{Deserialize, Serialize};

use crate::Terrains;
use crate::chunk::ChunkSource;
use crate::cube::NodeKey;
use crate::noise::{cell_random, noise3};

/// Stones and boulders strewn over a terrain (see the module docs). Off by default; the
/// Moon's and Mars's terrains have them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RockField {
    /// Stones over 12 cm across per 100 m² on the rockiest ground (0 = no rocks).
    pub per_100m2: f64,
    /// 0..1 share of the ground that is rocky (in patches a few hundred meters across).
    pub coverage: f64,
    /// Steepness of the size law: the count of rocks wider than D falls as D^−exponent.
    pub exponent: f64,
    /// Widest boulder, meters.
    pub max_size_m: f64,
    /// Linear base color, and how much single rocks vary in brightness (0..1).
    pub color: [f32; 3],
    pub variation: f32,
    /// The stone's surface texture, tinted by `color` (none: plain stone, its facets and
    /// each rock's shade carry it).
    pub texture: Option<MaterialPreset>,
}

impl Default for RockField {
    fn default() -> Self {
        Self {
            per_100m2: 0.0,
            coverage: 0.5,
            exponent: 2.3,
            max_size_m: 4.0,
            color: [0.2, 0.2, 0.2],
            variation: 0.25,
            texture: Some(MaterialPreset::Rock),
        }
    }
}

impl RockField {
    /// Grey stones and a few boulders on the regolith, rather fewer than on Mars.
    pub fn moon() -> Self {
        Self {
            per_100m2: 25.0,
            coverage: 0.55,
            exponent: 2.2,
            max_size_m: 5.0,
            color: [0.21, 0.205, 0.195],
            variation: 0.3,
            // Plain grey: Earth's rock textures bring lichen and earthy stains.
            texture: None,
        }
    }

    /// Dark, dusty basalt strewn thickly over the red plains, as Curiosity sees them.
    pub fn mars() -> Self {
        Self {
            per_100m2: 60.0,
            coverage: 0.7,
            exponent: 2.4,
            max_size_m: 4.0,
            color: [0.16, 0.085, 0.052],
            variation: 0.35,
            texture: Some(MaterialPreset::Rock),
        }
    }

    /// Rocks wider than `size_m` per square meter on the rockiest ground.
    fn density_over(&self, size_m: f64) -> f64 {
        self.per_100m2 / 100.0 * (size_m / SMALLEST).powf(-self.exponent)
    }
}

/// Narrowest stone, meters.
const SMALLEST: f64 = 0.12;

/// A tier of rock cells: the sizes it holds, its cell size and how far out it reaches.
#[derive(Debug, Clone, Copy)]
struct Tier {
    sizes: (f64, f64),
    /// Icosphere subdivisions of its rock shapes (1: 80 facets, 2: 320).
    detail: usize,
    cell_m: f64,
    reach_m: f64,
}

const STONES: Tier = Tier {
    sizes: (SMALLEST, 0.6),
    detail: 1,
    cell_m: 24.0,
    reach_m: 110.0,
};
const BOULDERS: Tier = Tier {
    sizes: (0.6, f64::INFINITY),
    detail: 2,
    cell_m: 96.0,
    reach_m: 450.0,
};
const TIERS: [Tier; 2] = [STONES, BOULDERS];

/// Most vertices in one cell's mesh (rocks past it are left out).
const MAX_CELL_VERTICES: usize = 60_000;

/// Cells being built at once.
const MAX_BUILDING: usize = 6;

/// One cell: terrain, quadtree node and tier.
type CellKey = (Entity, NodeKey, u8);

/// A cell's rocks, in the cell's tangent frame.
struct CellMesh {
    /// Terrain-local origin (on the ground at the cell's center) and rotation (+Y up).
    origin: DVec3,
    rotation: DQuat,
    mesh: Option<Mesh>,
}

struct Cell {
    entity: Option<Entity>,
    mesh: Option<Handle<Mesh>>,
    origin: DVec3,
    rotation: DQuat,
}

/// The rock cells in the world.
#[derive(Resource, Default)]
pub struct RockCells {
    cells: HashMap<CellKey, Cell>,
    building: HashMap<CellKey, JoinHandle<CellMesh>>,
    /// Each terrain's rock material, its version and pose (cells follow it when it moves), and
    /// how many of its edits the cells have seen.
    terrains: HashMap<Entity, TerrainRocks>,
    /// Cells put in place since startup.
    pub placed: u64,
}

struct TerrainRocks {
    material: Handle<StandardMaterial>,
    version: u64,
    pose: (DVec3, DQuat),
    seen_edits: usize,
}

impl RockCells {
    /// Cells with rocks drawn.
    pub fn drawn(&self) -> usize {
        self.cells.values().filter(|c| c.entity.is_some()).count()
    }

    /// Nothing waiting to be built.
    pub fn is_settled(&self) -> bool {
        self.building.is_empty()
    }
}

/// Keep rock cells around every focus point near a terrain with a [`RockField`].
#[allow(clippy::too_many_arguments)]
pub fn update_rocks(
    mut commands: Commands,
    terrains: Res<Terrains>,
    focus: Res<FocusPoints>,
    meshes: Option<ResMut<Assets<Mesh>>>,
    materials: Option<ResMut<Assets<StandardMaterial>>>,
    mut state: ResMut<RockCells>,
    diagnostics: Option<ResMut<engine_core::diagnostics::Diagnostics>>,
) {
    let (Some(mut meshes), Some(mut materials)) = (meshes, materials) else {
        return;
    };
    let state = &mut *state;
    let mut wanted: Vec<(f64, CellKey)> = Vec::new();
    for (&e, t) in &terrains.entries {
        let field = &t.config.rocks;
        if field.per_100m2 <= 0.0 || !t.source.map.is_sphere() {
            continue;
        }
        // A new look (or shape) starts over; a move carries the cells along.
        let pose = (t.position, t.rotation);
        let entry = state.terrains.entry(e).or_insert_with(|| TerrainRocks {
            material: materials.add(material(field)),
            version: t.version,
            pose,
            seen_edits: 0,
        });
        if entry.version != t.version {
            entry.version = t.version;
            if let Some(m) = materials.get_mut(entry.material) {
                *m = material(field);
            }
            let stale: Vec<CellKey> = state.cells.keys().filter(|k| k.0 == e).copied().collect();
            for k in stale {
                drop_cell(&mut commands, &mut meshes, state.cells.remove(&k));
            }
            state.building.retain(|k, _| k.0 != e);
        }
        let moved = entry.pose != pose;
        entry.pose = pose;
        // Edits (craters, trenches) rebuild the cells they touch.
        if entry.seen_edits < t.edit_log.len() {
            let edits = t.edit_log[entry.seen_edits..].to_vec();
            entry.seen_edits = t.edit_log.len();
            let touched: Vec<CellKey> = state
                .cells
                .iter()
                .filter(|(k, c)| {
                    k.0 == e
                        && edits.iter().any(|&(at, r)| {
                            (at - c.origin).length() < r + TIERS[k.2 as usize].cell_m
                        })
                })
                .map(|(k, _)| *k)
                .collect();
            for k in touched {
                drop_cell(&mut commands, &mut meshes, state.cells.remove(&k));
            }
        }
        if moved {
            for (_, c) in state.cells.iter().filter(|(k, _)| k.0 == e) {
                if let Some(entity) = c.entity {
                    commands
                        .entity(entity)
                        .insert(place(pose, c.origin, c.rotation));
                }
            }
        }
        for &p in &focus.points {
            if t.altitude_hint(p) > BOULDERS.reach_m + t.config.height_range().1 {
                continue;
            }
            let local = t.to_local(p);
            let up = local.normalize_or(DVec3::Y);
            let side = up.any_orthonormal_vector();
            let fwd = up.cross(side);
            for (tier_i, tier) in TIERS.iter().enumerate() {
                let level = level_for(&t.source, tier.cell_m);
                let size = t.source.map.node_size(level);
                let n = (tier.reach_m / size).ceil() as i32 + 1;
                for dz in -n..=n {
                    for dx in -n..=n {
                        let q = local + (side * dx as f64 + fwd * dz as f64) * size;
                        let Some(key) = t.source.map.node_at(q, level) else {
                            continue;
                        };
                        let (lo, s) = key.face_rect();
                        let mid =
                            t.source
                                .map
                                .position(key.face, lo.x + s * 0.5, lo.y + s * 0.5, 0.0);
                        // Along the ground from the focus to the cell's nearest edge.
                        let d = (mid.normalize() * local.length() - local).length() - size * 0.7;
                        if d <= tier.reach_m {
                            wanted.push((d.max(0.0), (e, key, tier_i as u8)));
                        }
                    }
                }
            }
        }
    }
    state
        .terrains
        .retain(|e, _| terrains.entries.contains_key(e));
    wanted.sort_by(|a, b| a.0.total_cmp(&b.0));
    let wanted_set: HashSet<CellKey> = wanted.iter().map(|w| w.1).collect();

    // Cells no longer wanted go; builds no longer wanted are left to finish and dropped.
    let stale: Vec<CellKey> = state
        .cells
        .keys()
        .filter(|k| !wanted_set.contains(k))
        .copied()
        .collect();
    for k in stale {
        drop_cell(&mut commands, &mut meshes, state.cells.remove(&k));
    }
    // Put finished cells in place.
    let done: Vec<CellKey> = state
        .building
        .iter()
        .filter(|(_, h)| h.is_finished())
        .map(|(k, _)| *k)
        .collect();
    for k in done {
        let Some(handle) = state.building.remove(&k) else {
            continue;
        };
        let Ok(built) = handle.join() else {
            continue;
        };
        if !wanted_set.contains(&k) {
            continue;
        }
        let Some(terrain) = state.terrains.get(&k.0) else {
            continue;
        };
        let mut cell = Cell {
            entity: None,
            mesh: None,
            origin: built.origin,
            rotation: built.rotation,
        };
        if let Some(mesh) = built.mesh {
            let h = meshes.add(mesh);
            cell.entity = Some(
                commands
                    .spawn((
                        Name::new("Rocks"),
                        place(terrain.pose, built.origin, built.rotation),
                        MeshRenderer {
                            mesh: MeshRef::Handle(h),
                            material: MaterialRef::Handle(terrain.material),
                            cast_shadows: true,
                            visible: true,
                        },
                        NoVirtualGeometry,
                        // The tiers choose stones by distance; the renderer's own
                        // simplification would drop whole stones near the camera.
                        NoMeshLod,
                    ))
                    .id(),
            );
            cell.mesh = Some(h);
        }
        state.cells.insert(k, cell);
        state.placed += 1;
    }
    // Start the nearest missing cells.
    for (_, k) in &wanted {
        if state.building.len() >= MAX_BUILDING {
            break;
        }
        if state.cells.contains_key(k) || state.building.contains_key(k) {
            continue;
        }
        let Some(t) = terrains.entries.get(&k.0) else {
            continue;
        };
        let (source, field, key, tier) = (t.source.clone(), t.config.rocks.clone(), k.1, k.2);
        let seed = t.config.shape.seed;
        state.building.insert(
            *k,
            std::thread::spawn(move || {
                build_cell(&source, &field, key, TIERS[tier as usize], seed)
            }),
        );
    }
    if let Some(mut d) = diagnostics {
        d.set("terrain.rock_cells", state.drawn() as f64);
        d.set("terrain.rock_builds", state.building.len() as f64);
    }
}

fn drop_cell(commands: &mut Commands, meshes: &mut Assets<Mesh>, cell: Option<Cell>) {
    let Some(cell) = cell else {
        return;
    };
    if let Some(e) = cell.entity {
        commands.entity(e).despawn();
    }
    if let Some(h) = cell.mesh {
        meshes.remove(h);
    }
}

fn place(pose: (DVec3, DQuat), origin: DVec3, rotation: DQuat) -> Transform {
    let (position, rot) = pose;
    Transform {
        translation: position + rot * origin,
        rotation: (rot * rotation).normalize().as_quat(),
        ..Default::default()
    }
}

/// The quadtree level whose nodes are about `cell_m` across.
fn level_for(source: &ChunkSource, cell_m: f64) -> u8 {
    let root = source.map.root_size();
    (root / cell_m.max(1.0)).log2().round().clamp(0.0, 24.0) as u8
}

fn material(field: &RockField) -> StandardMaterial {
    let [r, g, b] = field.color;
    match field.texture {
        // The preset's own albedo is about mid grey: lift the tint to land on `color`.
        Some(texture) => StandardMaterial {
            base_color: [r * 2.2, g * 2.2, b * 2.2, 1.0],
            perceptual_roughness: 0.92,
            ..StandardMaterial::preset(texture).with_triplanar()
        },
        None => StandardMaterial {
            base_color: [r, g, b, 1.0],
            perceptual_roughness: 0.95,
            ..Default::default()
        },
    }
}

/// How rocky the ground is at a terrain-local direction (0..1): patches a few hundred meters
/// across, and a few stones nearly everywhere.
fn rockiness(field: &RockField, dir: DVec3, radius: f64, seed: u64, slope: f64) -> f64 {
    let p = dir * radius;
    let patches =
        noise3(p / 320.0, seed ^ 0x5_eed0_f0cc) * 0.7 + noise3(p / 90.0, seed ^ 0xb01d) * 0.3;
    // Coverage moves the threshold: at 0.5 about half the ground is rocky.
    let t = ((patches + 0.6 * (field.coverage - 0.5) * 2.0 + 0.35) / 0.7).clamp(0.0, 1.0);
    let floor = 0.15 + 0.5 * field.coverage;
    (floor + (1.0 - floor) * t * t * (3.0 - 2.0 * t) + slope * 2.5).min(1.0)
}

/// The rocks of one cell.
fn build_cell(
    source: &ChunkSource,
    field: &RockField,
    key: NodeKey,
    tier: Tier,
    seed: u64,
) -> CellMesh {
    let map = &source.map;
    let (lo, s) = key.face_rect();
    let ground = |local: DVec3| -> Option<(DVec3, DVec3)> {
        let (dir, up, base) = map.locate(local)?;
        Some((base + up * source.sample(base, up, dir, 0.0).height, up))
    };
    let center = map.position(key.face, lo.x + s * 0.5, lo.y + s * 0.5, 0.0);
    let (origin, up) = ground(center).unwrap_or((center, center.normalize()));
    let rotation = DQuat::from_rotation_arc(DVec3::Y, up);
    let inv = rotation.inverse();
    let size = map.node_size(key.level);
    let (small, big) = (tier.sizes.0, tier.sizes.1.min(field.max_size_m));
    if big <= small {
        return CellMesh {
            origin,
            rotation,
            mesh: None,
        };
    }
    let expected = (field.density_over(small) - field.density_over(big)) * size * size;
    let salt = ((key.face as u64) << 56) ^ ((key.level as u64) << 48) ^ (tier.cell_m as u64);
    let rand = |i: u64, j: u64| cell_random(key.x as i64, key.y as i64, i as i64, seed, salt ^ j);
    // A whole number of candidates with the right mean.
    let count = expected.floor() as u64 + u64::from(rand(0, 1) < expected.fract());
    let shapes = rock_shapes(tier.detail);
    let (mut vertices, mut indices) = (Vec::new(), Vec::new());
    let (ea, eb) = (small.powf(-field.exponent), big.powf(-field.exponent));
    for i in 1..=count {
        let local = map.position(key.face, lo.x + s * rand(i, 2), lo.y + s * rand(i, 3), 0.0);
        // Size from the power law (inverse of its cumulative count).
        let d = (ea - rand(i, 4) * (ea - eb)).powf(-1.0 / field.exponent);
        let Some((point, up)) = ground(local) else {
            continue;
        };
        // The ground's tilt under the rock, from two samples half its size away.
        let t1 = up.any_orthonormal_vector();
        let t2 = up.cross(t1);
        let e = (d * 0.5).max(0.3);
        let (Some((pa, _)), Some((pb, _))) = (ground(local + t1 * e), ground(local + t2 * e))
        else {
            continue;
        };
        let normal = (pa - point).cross(pb - point).normalize_or(up);
        let normal = if normal.dot(up) < 0.0 {
            -normal
        } else {
            normal
        };
        let slope = 1.0 - normal.dot(up);
        if rand(i, 5) > rockiness(field, up, map.radius, seed, slope) {
            continue;
        }
        // In the cell's frame: tilted most of the way to the ground, turned at random, sunk
        // a little (its center is at mid height: stones a few centimeters tall must keep
        // nearly all of it above the ground to show).
        let n = (inv * normal).as_vec3();
        let lean = Vec3::Y.lerp(n, 0.8).normalize();
        let tumble = Quat::from_axis_angle(
            Vec3::new(rand(i, 6) as f32 - 0.5, 0.0, rand(i, 7) as f32 - 0.5).normalize_or(Vec3::X),
            (rand(i, 8) as f32 - 0.5) * 0.5,
        );
        let q = Quat::from_rotation_arc(Vec3::Y, lean)
            * Quat::from_rotation_y(rand(i, 9) as f32 * std::f32::consts::TAU)
            * tumble;
        let shape = &shapes[(rand(i, 10) * shapes.len() as f64) as usize % shapes.len()];
        if vertices.len() + shape.vertices.len() > MAX_CELL_VERTICES {
            break;
        }
        let flat = 0.6 + rand(i, 11) as f32 * 0.4;
        let scale = Vec3::new(1.0, flat, 0.7 + rand(i, 12) as f32 * 0.3) * d as f32;
        let at =
            (inv * (point - origin)).as_vec3() - n * (scale.y * (0.02 + rand(i, 13) as f32 * 0.08));
        let shade = 1.0 + (rand(i, 14) as f32 - 0.5) * 2.0 * field.variation;
        let color = Vec4::new(shade, shade, shade * (1.0 - 0.04 * (shade - 1.0)), 1.0);
        let start = vertices.len() as u32;
        for v in &shape.vertices {
            let p = Vec3::from(v.position) * scale;
            let nrm = (Vec3::from(v.normal) / scale).normalize_or(Vec3::Y);
            vertices.push(Vertex::new(at + q * p, q * nrm, Vec2::from(v.uv)).with_color(color));
        }
        indices.extend(shape.indices.iter().map(|i| start + i));
    }
    CellMesh {
        origin,
        rotation,
        mesh: (!vertices.is_empty()).then(|| Mesh::new(vertices, indices)),
    }
}

/// A few fractured-stone shapes, one meter across (a lumpy sphere cut by planes into facets).
/// `detail`: 1 for stones (80 facets), 2 for boulders (320).
fn rock_shapes(detail: usize) -> &'static [Mesh] {
    static SHAPES: [OnceLock<Vec<Mesh>>; 3] = [OnceLock::new(), OnceLock::new(), OnceLock::new()];
    let detail = detail.min(2);
    SHAPES[detail].get_or_init(|| (0..8).map(|v| rock_shape(v, detail as u32)).collect())
}

fn rock_shape(variant: u64, detail: u32) -> Mesh {
    let (dirs, tris) = icosphere(detail);
    let r = |j: u64| cell_random(variant as i64, 0, 0, 0x0c0_ffee, j);
    let cuts: Vec<(DVec3, f64)> = (0..5 + (r(0) * 5.0) as u64)
        .map(|j| {
            let n = DVec3::new(r(j * 4 + 1) - 0.5, r(j * 4 + 2) - 0.5, r(j * 4 + 3) - 0.5)
                .normalize_or(DVec3::X);
            (n, 0.32 + 0.14 * r(j * 4 + 4))
        })
        .collect();
    let positions: Vec<Vec3> = dirs
        .iter()
        .map(|&d| {
            let mut radius =
                0.5 * (1.0 + 0.18 * noise3(d * 1.7, variant) + 0.05 * noise3(d * 5.0, variant + 9));
            for &(n, o) in &cuts {
                let k = d.dot(n);
                if k > 1e-3 {
                    radius = radius.min(o / k);
                }
            }
            (d * radius).as_vec3()
        })
        .collect();
    // One closed, welded surface with smooth normals. Normals lean toward each facet's own,
    // so cut planes read flat.
    let mut normals = vec![Vec3::ZERO; positions.len()];
    for t in &tris {
        let [a, b, c] = [positions[t[0]], positions[t[1]], positions[t[2]]];
        let n = (b - a).cross(c - a);
        for &i in t {
            normals[i] += n;
        }
    }
    let vertices: Vec<Vertex> = positions
        .iter()
        .zip(&normals)
        .map(|(&p, &n)| {
            let n = n.normalize_or(p.normalize_or(Vec3::Y));
            Vertex::new(p, n, Vec2::new(p.x + 0.5, p.z + 0.5))
        })
        .collect();
    let indices: Vec<u32> = tris.iter().flatten().map(|&i| i as u32).collect();
    Mesh::new(vertices, indices)
}

/// Unit icosphere: directions and triangles (counter-clockwise from outside).
fn icosphere(subdivisions: u32) -> (Vec<DVec3>, Vec<[usize; 3]>) {
    let t = (1.0 + 5f64.sqrt()) / 2.0;
    let mut dirs: Vec<DVec3> = [
        (-1.0, t, 0.0),
        (1.0, t, 0.0),
        (-1.0, -t, 0.0),
        (1.0, -t, 0.0),
        (0.0, -1.0, t),
        (0.0, 1.0, t),
        (0.0, -1.0, -t),
        (0.0, 1.0, -t),
        (t, 0.0, -1.0),
        (t, 0.0, 1.0),
        (-t, 0.0, -1.0),
        (-t, 0.0, 1.0),
    ]
    .iter()
    .map(|&(x, y, z)| DVec3::new(x, y, z).normalize())
    .collect();
    let mut tris: Vec<[usize; 3]> = vec![
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ];
    for _ in 0..subdivisions {
        let mut mid: HashMap<(usize, usize), usize> = HashMap::new();
        let mut split = |a: usize, b: usize, dirs: &mut Vec<DVec3>| {
            *mid.entry((a.min(b), a.max(b))).or_insert_with(|| {
                dirs.push((dirs[a] + dirs[b]).normalize());
                dirs.len() - 1
            })
        };
        let mut next = Vec::with_capacity(tris.len() * 4);
        for [a, b, c] in tris {
            let ab = split(a, b, &mut dirs);
            let bc = split(b, c, &mut dirs);
            let ca = split(c, a, &mut dirs);
            next.extend([[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]);
        }
        tris = next;
    }
    (dirs, tris)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PlanetTerrain;

    #[test]
    fn rock_shapes_are_closed_and_about_a_meter() {
        for m in rock_shapes(1).iter().chain(rock_shapes(2)) {
            let a = m.aabb();
            let size = a.max - a.min;
            assert!(
                size.max_element() <= 1.05 && size.min_element() > 0.5,
                "{size:?}"
            );
            // Facets face outward.
            for t in m.indices.chunks(3) {
                let p = |i: u32| Vec3::from(m.vertices[i as usize].position);
                let n = (p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0]));
                assert!(n.dot(p(t[0]) + p(t[1]) + p(t[2])) > -1e-6);
            }
        }
    }

    #[test]
    fn sizes_follow_the_power_law() {
        let f = RockField::mars();
        // Ten times wider: a 10^2.4th as many.
        let ratio = f.density_over(0.12) / f.density_over(1.2);
        assert!((ratio - 10f64.powf(2.4)).abs() < 1.0, "{ratio}");
    }

    #[test]
    fn mars_cells_hold_stones_on_the_ground_and_the_same_ones_every_time() {
        let t = PlanetTerrain::mars(3);
        let source = crate::TerrainRuntime::new(&t, Default::default(), 1).source;
        let level = level_for(&source, STONES.cell_m);
        // Somewhere on the plains.
        let local = crate::dir_from_lat_lon(-4.6, 137.4) * source.map.radius;
        let key = source.map.node_at(local, level).unwrap();
        let a = build_cell(&source, &t.rocks, key, STONES, 3);
        let b = build_cell(&source, &t.rocks, key, STONES, 3);
        let (ma, mb) = (a.mesh.expect("stones"), b.mesh.expect("stones"));
        assert_eq!(ma.vertices.len(), mb.vertices.len());
        assert_eq!(ma.vertices[0].position, mb.vertices[0].position);
        let rocks = ma.vertices.len() / rock_shapes(STONES.detail)[0].vertices.len();
        assert!((3..400).contains(&rocks), "{rocks} stones");
        // Every stone sits on the ground, sunk in a little: none floats a meter up.
        let up_most = ma
            .vertices
            .iter()
            .map(|v| v.position[1])
            .fold(f32::MIN, f32::max);
        let half = source.map.node_size(level) as f32;
        assert!(up_most < half, "{up_most}");
    }
}
