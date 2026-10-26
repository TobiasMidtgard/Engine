//! Physics colliders for terrain: heightfield tiles around the simulation focus.
//!
//! Tiles are quadtree patches of about [`TerrainSettings::collider_tile_m`] on the terrain,
//! so the tiling is stable anywhere on a planet. Each tile is a static entity with a
//! [`Collider`](engine_world::physics::Collider) `Heightfield` in the tile's tangent frame
//! (local +Y = the planet's up at the tile center). Heights are at full detail, including
//! edits. Tiles are rebuilt when an edit touches them. Tiles cover the 3 × 3 block around
//! every focus point and the tile under every dynamic body.
//!
//! A tile takes thousands of full terrain samples (over 100 ms on a showcase planet), so
//! tiles are built on worker threads, a few at a time, and put in place when done: the frame
//! never waits for them. Only the tiles right under a focus point or a body in the simulated
//! range ([`SimulationRange`]), when they have none (at the start, or after a jump), are built
//! on the spot, side by side on all cores, so nothing stands on thin air.
//!
//! Heightfield layout: `heights[row * cols + col]`, rows along local Z from −size/2 to
//! +size/2, columns along local X from −size/2 to +size/2, heights in meters above the tile
//! origin.

use std::collections::{HashMap, HashSet};
use std::thread::JoinHandle;

use engine_core::math::{DMat3, DQuat, DVec3};
use engine_core::prelude::*;
use engine_world::physics::{BodyKind, Collider, ColliderShape, RigidBody};
use engine_world::{FocusPoints, SimulationRange, SurfaceMaterials};

use crate::chunk::ChunkSource;
use crate::cube::NodeKey;
use crate::palette::{SurfaceIds, TerrainPalette};
use crate::{TerrainRuntime, TerrainSettings, Terrains};

/// Marks a terrain collider tile entity.
#[derive(Component, Debug, Clone, Copy)]
pub struct TerrainColliderTile {
    pub terrain: Entity,
    pub key: NodeKey,
}

/// Collider tiles in the world.
#[derive(Resource, Debug, Default)]
pub struct TerrainColliders {
    tiles: HashMap<(Entity, NodeKey), Entity>,
    /// How far each terrain's edit log has been applied.
    seen_edits: HashMap<Entity, usize>,
    /// Tiles waiting to be (re)built.
    queue: Vec<(Entity, NodeKey)>,
    /// Tiles being built on worker threads.
    building: HashMap<(Entity, NodeKey), JoinHandle<TileShape>>,
    /// Tiles built so far (diagnostics: a count that keeps climbing while nothing moves is
    /// tiles being thrown away and built again).
    built: u64,
}

impl TerrainColliders {
    pub fn len(&self) -> usize {
        self.tiles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }

    /// No tile waiting or being built.
    pub fn is_settled(&self) -> bool {
        self.queue.is_empty() && self.building.is_empty()
    }
}

/// Level whose patches are closest to `tile_m` across.
fn tile_level(t: &TerrainRuntime, tile_m: f64) -> u8 {
    let root = t.source.map.root_size();
    (root / tile_m.max(1.0)).log2().round().clamp(0.0, 24.0) as u8
}

/// A tile's heightfield and where it lies in its terrain's own frame (so a tile built while
/// the world shifted or the terrain moved is put in the right place).
pub struct TileShape {
    /// Terrain-local origin (on the ground at the tile's center) and axes.
    origin: DVec3,
    rotation: DQuat,
    collider: Collider,
}

impl TileShape {
    /// World transform of the tile on `t`.
    fn transform(&self, t: &TerrainRuntime) -> Transform {
        Transform::from_translation(t.position + t.rotation * self.origin)
            .with_rotation((t.rotation * self.rotation).as_quat())
    }
}

/// Build the heightfield for one tile: (world transform, collider).
pub fn build_tile(
    t: &TerrainRuntime,
    key: NodeKey,
    samples: u32,
    ids: &SurfaceIds,
) -> (Transform, Collider) {
    let shape = tile_shape(&t.source, &t.config.palette, key, samples, ids);
    (shape.transform(t), shape.collider)
}

/// The heightfield for one tile, in the terrain's frame.
fn tile_shape(
    source: &ChunkSource,
    palette: &TerrainPalette,
    key: NodeKey,
    samples: u32,
    ids: &SurfaceIds,
) -> TileShape {
    let map = &source.map;
    let (lo, size) = key.face_rect();
    let center_ab = lo + size * 0.5;
    let (base_c, up, dir_c) = map.base(key.face, center_ab.x, center_ab.y);
    let center = source.sample(base_c, up, dir_c, 0.0);
    // The tile origin sits on the ground at the tile center, so heights stay small.
    let origin = base_c + up * center.height;
    // Tangent axes along the face's a and b directions.
    let pa = map.base(key.face, center_ab.x + size * 0.01, center_ab.y).0;
    let pb = map.base(key.face, center_ab.x, center_ab.y + size * 0.01).0;
    let ax = (pa - origin).normalize();
    let x_axis = (ax - up * ax.dot(up)).normalize();
    let z_axis = x_axis.cross(up); // right-handed: x × y = z
    let _ = pb;
    let n = samples.max(2);
    let step = size / (n - 1) as f64;
    let mut heights = vec![0.0f32; (n * n) as usize];
    // Rows run along +local Z = −b (b increases toward −Z because z = x × y).
    let b_sign = if (pb - origin).dot(z_axis) > 0.0 {
        1.0
    } else {
        -1.0
    };
    let mut extent_x: f64 = 0.0;
    let mut extent_z: f64 = 0.0;
    for row in 0..n {
        for col in 0..n {
            let a = lo.x + col as f64 * step;
            let b = if b_sign > 0.0 {
                lo.y + row as f64 * step
            } else {
                lo.y + size - row as f64 * step
            };
            let (base, u, dir) = map.base(key.face, a, b);
            let s = source.sample(base, u, dir, 0.0);
            let p = base + u * s.height - origin;
            heights[(row * n + col) as usize] = p.dot(up) as f32;
            extent_x = extent_x.max(p.dot(x_axis).abs());
            extent_z = extent_z.max(p.dot(z_axis).abs());
        }
    }
    let surface = palette.classify(&center, 0.0, center.height < 0.0 && source.shape.ocean, ids);
    let collider = Collider {
        shape: ColliderShape::Heightfield {
            heights,
            rows: n,
            cols: n,
            size: [(extent_x * 2.0) as f32, (extent_z * 2.0) as f32],
        },
        surface,
        ..Default::default()
    };
    TileShape {
        origin,
        rotation: DQuat::from_mat3(&DMat3::from_cols(x_axis, up, z_axis)),
        collider,
    }
}

/// Most tiles kept under dynamic bodies away from every focus point, nearest first.
pub const MAX_BODY_TILES: usize = 64;
/// Most tiles built at once on worker threads.
const MAX_BUILDING: usize = 3;

/// Keep collider tiles on every terrain that wants colliders: the 3 × 3 block around every
/// focus point, and the tile under every dynamic body (awake or asleep, so a crate left
/// behind still has ground when something wakes it), up to [`MAX_BODY_TILES`] of those.
#[allow(clippy::too_many_arguments)]
pub fn update_colliders(
    mut commands: Commands,
    terrains: Res<Terrains>,
    settings: Res<TerrainSettings>,
    focus: Res<FocusPoints>,
    materials: Res<SurfaceMaterials>,
    bodies: Query<(&Transform, &RigidBody)>,
    range: Option<Res<SimulationRange>>,
    mut state: ResMut<TerrainColliders>,
    diagnostics: Option<ResMut<engine_core::diagnostics::Diagnostics>>,
) {
    let range = range.map(|r| *r).unwrap_or_default();
    let body_points: Vec<DVec3> = bodies
        .iter()
        .filter(|(_, b)| b.kind == BodyKind::Dynamic)
        .map(|(tf, _)| tf.translation)
        .collect();
    if focus.points.is_empty() && body_points.is_empty() {
        return;
    }
    // Wanted tiles in build order: around the focus points first, then under bodies.
    let mut wanted: Vec<(Entity, NodeKey)> = Vec::new();
    let mut wanted_set: HashSet<(Entity, NodeKey)> = HashSet::new();
    let mut body_tiles: Vec<(f64, (Entity, NodeKey))> = Vec::new();
    // The tiles right under the focus points and under bodies in the simulated range: built on
    // the spot when missing, so nothing falls through.
    let mut underfoot: Vec<(Entity, NodeKey)> = Vec::new();
    for (&e, t) in &terrains.entries {
        if !t.config.colliders {
            continue;
        }
        let level = tile_level(t, settings.collider_tile_m);
        let size = t.source.map.node_size(level);
        // Only near the surface (within 2 km).
        let near = |p: DVec3| t.altitude_hint(p).abs() <= 2_000.0 + t.config.height_range().1;
        for &p in focus.points.iter().filter(|&&p| near(p)) {
            let local = t.to_local(p);
            let Some((_, up, _)) = t.source.map.locate(local) else {
                continue;
            };
            let side = up.any_orthonormal_vector();
            let fwd = up.cross(side);
            if let Some(k) = t.source.map.node_at(local, level) {
                underfoot.push((e, k));
            }
            for dz in -1..=1 {
                for dx in -1..=1 {
                    let q = local + (side * dx as f64 + fwd * dz as f64) * size;
                    if let Some(k) = t.source.map.node_at(q, level)
                        && wanted_set.insert((e, k))
                    {
                        wanted.push((e, k));
                    }
                }
            }
        }
        for &p in body_points.iter().filter(|&&p| near(p)) {
            if let Some(k) = t.source.map.node_at(t.to_local(p), level) {
                body_tiles.push((focus.distance(p), (e, k)));
                if range.is_active(&focus, p) {
                    underfoot.push((e, k));
                }
            }
        }
        // Rebuild tiles touched by new edits.
        let seen = state.seen_edits.get(&e).copied().unwrap_or(0);
        if seen < t.edit_log.len() {
            let mut touched = Vec::new();
            for &(c, r) in &t.edit_log[seen..] {
                for &(te, key) in state.tiles.keys() {
                    if te != e {
                        continue;
                    }
                    let (lo, s) = key.face_rect();
                    let mid = lo + s * 0.5;
                    let tc = t.source.map.position(key.face, mid.x, mid.y, 0.0);
                    if (tc - c).length() < r + size && !touched.contains(&(te, key)) {
                        touched.push((te, key));
                    }
                }
            }
            state.queue.extend(touched);
            state.seen_edits.insert(e, t.edit_log.len());
        }
    }
    body_tiles.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut extra = 0;
    for (_, k) in body_tiles {
        if extra >= MAX_BODY_TILES {
            break;
        }
        if wanted_set.insert(k) {
            wanted.push(k);
            extra += 1;
        }
    }
    // Remove tiles no longer wanted.
    let stale: Vec<_> = state
        .tiles
        .keys()
        .filter(|k| !wanted_set.contains(k))
        .copied()
        .collect();
    for k in stale {
        if let Some(entity) = state.tiles.remove(&k) {
            commands.entity(entity).despawn();
        }
    }
    // Edits first, then missing tiles in the order they are wanted.
    state.queue.retain(|k| wanted_set.contains(k));
    let mut queued: HashSet<(Entity, NodeKey)> = HashSet::new();
    state.queue.retain(|k| queued.insert(*k));
    for k in &wanted {
        if !state.tiles.contains_key(k) && queued.insert(*k) {
            state.queue.push(*k);
        }
    }
    let ids = SurfaceIds::new(&materials);
    // Put finished tiles in place (or drop them, if no longer wanted).
    let done: Vec<_> = state
        .building
        .iter()
        .filter(|(_, h)| h.is_finished())
        .map(|(&k, _)| k)
        .collect();
    for k in done {
        let handle = state.building.remove(&k).expect("tile being built");
        let Ok(shape) = handle.join() else {
            continue;
        };
        place(&mut commands, &terrains, &mut state, &wanted_set, k, shape);
    }
    // Missing ground underfoot: build it now (waiting for tiles already on their way), all
    // at once across the cores.
    let mut missing: Vec<(Entity, NodeKey)> = Vec::new();
    for k in underfoot {
        if state.tiles.contains_key(&k) || missing.contains(&k) || !wanted_set.contains(&k) {
            continue;
        }
        if let Some(handle) = state.building.remove(&k)
            && let Ok(shape) = handle.join()
        {
            place(&mut commands, &terrains, &mut state, &wanted_set, k, shape);
            continue;
        }
        missing.push(k);
    }
    if !missing.is_empty() {
        let samples = settings.collider_samples;
        let shapes: Vec<_> = std::thread::scope(|scope| {
            let jobs: Vec<_> = missing
                .iter()
                .filter_map(|&k| {
                    let t = terrains.entries.get(&k.0)?;
                    let ids = &ids;
                    Some((
                        k,
                        scope.spawn(move || {
                            tile_shape(&t.source, &t.config.palette, k.1, samples, ids)
                        }),
                    ))
                })
                .collect();
            jobs.into_iter()
                .filter_map(|(k, job)| job.join().ok().map(|shape| (k, shape)))
                .collect()
        });
        for (k, shape) in shapes {
            state.queue.retain(|q| *q != k);
            place(&mut commands, &terrains, &mut state, &wanted_set, k, shape);
        }
    }
    // Start the queued tiles on worker threads, a few at a time. (A tile already on its way
    // waits in the queue: an edit since it started needs a fresh build after it.)
    let mut i = 0;
    while state.building.len() < MAX_BUILDING && i < state.queue.len() {
        let k = state.queue[i];
        if state.building.contains_key(&k) {
            i += 1;
            continue;
        }
        state.queue.remove(i);
        let Some(t) = terrains.entries.get(&k.0) else {
            continue;
        };
        let (source, palette, ids) = (t.source.clone(), t.config.palette.clone(), ids.clone());
        let samples = settings.collider_samples;
        let handle = std::thread::Builder::new()
            .name("terrain-collider".into())
            .spawn(move || tile_shape(&source, &palette, k.1, samples, &ids))
            .expect("spawn a terrain collider thread");
        state.building.insert(k, handle);
    }
    if let Some(mut d) = diagnostics {
        d.set("terrain.collider_tiles", state.tiles.len() as f64);
        d.set("terrain.collider_builds", state.built as f64);
    }
}

/// Put a built tile in place: a new tile entity, or the new shape on the tile it replaces.
fn place(
    commands: &mut Commands,
    terrains: &Terrains,
    state: &mut TerrainColliders,
    wanted: &HashSet<(Entity, NodeKey)>,
    k: (Entity, NodeKey),
    shape: TileShape,
) {
    let Some(t) = terrains.entries.get(&k.0) else {
        return;
    };
    if !wanted.contains(&k) {
        return;
    }
    state.built += 1;
    let transform = shape.transform(t);
    if let Some(old) = state.tiles.get(&k) {
        commands.entity(*old).insert((transform, shape.collider));
    } else {
        let id = commands
            .spawn((
                transform,
                shape.collider,
                TerrainColliderTile {
                    terrain: k.0,
                    key: k.1,
                },
            ))
            .id();
        state.tiles.insert(k, id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FlatPad, PlanetTerrain};
    use engine_core::math::DVec3;

    #[test]
    fn tile_heights_match_the_ground() {
        let pad = FlatPad::default();
        let config = PlanetTerrain::earthlike(2).with_pad(pad);
        let mut t = TerrainRuntime::new(&config, Default::default(), 1);
        let tf = config.home_transform(pad);
        t.position = tf.translation;
        t.rotation = tf.rotation.as_dquat().normalize();
        let level = tile_level(&t, 64.0);
        let key = t
            .source
            .map
            .node_at(t.to_local(DVec3::new(0.0, 0.0, 0.0)), level)
            .unwrap();
        let ids = SurfaceIds::new(&SurfaceMaterials::default());
        let (tf, collider) = build_tile(&t, key, 17, &ids);
        let ColliderShape::Heightfield { heights, size, .. } = collider.shape else {
            panic!("not a heightfield");
        };
        // On the flat pad every height is ~0 and the tile lies flat in the world.
        assert!(heights.iter().all(|h| h.abs() < 0.05), "{heights:?}");
        assert!((tf.rotation * Vec3::Y).y > 0.9999);
        assert!(size[0] > 30.0 && size[0] < 120.0, "{size:?}");
        assert!(tf.translation.y.abs() < 0.05);
    }

    fn app_with_planet() -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins(engine_world::WorldPlugin)
            .add_plugins(crate::TerrainPlugin);
        let pad = FlatPad::default();
        let t = PlanetTerrain::earthlike(3).with_pad(pad);
        app.world.spawn((t.home_transform(pad), t));
        let focus = app
            .world
            .spawn((
                Transform::from_xyz(0.0, 2.0, 0.0),
                engine_world::SimulationFocus::default(),
            ))
            .id();
        (app, focus)
    }

    /// Run frames until every wanted tile is built (the builds run on worker threads).
    fn settle(app: &mut App) {
        let start = std::time::Instant::now();
        app.update();
        while !app.world.resource::<TerrainColliders>().is_settled() {
            assert!(
                start.elapsed().as_secs() < 60,
                "collider tiles never settled"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
            app.update();
        }
        // (The last tiles' entities spawn with that frame's commands.)
        app.update();
    }

    /// Whether a collider tile lies under the point (x, z).
    fn tile_under(app: &mut App, x: f64, z: f64) -> bool {
        let mut q = app
            .world
            .query::<(&Transform, &Collider, &TerrainColliderTile)>();
        q.iter(&app.world).any(|(tf, c, _)| {
            let ColliderShape::Heightfield { size, .. } = c.shape else {
                return false;
            };
            let d = tf.translation - DVec3::new(x, tf.translation.y, z);
            d.x.abs() < size[0] as f64 * 0.5 && d.z.abs() < size[1] as f64 * 0.5
        })
    }

    #[test]
    fn bodies_away_from_the_focus_keep_ground_under_them() {
        let (mut app, focus) = app_with_planet();
        // A crate dropped a kilometer from the focus, and one left lying in the far distance.
        app.world.spawn((
            Transform::from_xyz(1_000.0, 30.0, 0.0),
            RigidBody::default(),
        ));
        app.world.spawn((
            Transform::from_xyz(0.0, 0.0, -1_500.0),
            RigidBody {
                kind: BodyKind::Kinematic,
                ..Default::default()
            },
        ));
        // Ground under the focus from the first frame.
        app.update();
        assert!(tile_under(&mut app, 0.0, 0.0), "no ground at the focus");
        settle(&mut app);
        assert!(
            tile_under(&mut app, 1_000.0, 0.0),
            "no ground under the crate"
        );
        // Kinematic bodies are moved by gameplay, not dropped: they get no tile of their own.
        assert!(!tile_under(&mut app, 0.0, -1_500.0));
        // The focus flies 2 km off: the crate keeps its ground, the old block goes.
        app.world.get_mut::<Transform>(focus).unwrap().translation = DVec3::new(-2_000.0, 2.0, 0.0);
        settle(&mut app);
        assert!(
            tile_under(&mut app, 1_000.0, 0.0),
            "the crate lost its ground"
        );
        assert!(
            tile_under(&mut app, -2_000.0, 0.0),
            "no ground at the new focus"
        );
        assert!(!tile_under(&mut app, 0.0, 0.0), "the old block stayed");
    }

    #[test]
    fn every_focus_point_gets_ground_and_body_tiles_are_capped() {
        let (mut app, _) = app_with_planet();
        app.world.spawn((
            Transform::from_xyz(3_000.0, 2.0, 0.0),
            engine_world::SimulationFocus::default(),
        ));
        // A scattered field of crates, far more than the cap.
        for i in 0..100 {
            let (x, z) = ((i % 10) as f64 * 150.0, 500.0 + (i / 10) as f64 * 150.0);
            app.world
                .spawn((Transform::from_xyz(x, 10.0, z), RigidBody::default()));
        }
        settle(&mut app);
        assert!(
            tile_under(&mut app, 3_000.0, 0.0),
            "no ground at the second focus"
        );
        let tiles = app.world.resource::<TerrainColliders>().len();
        assert_eq!(tiles, 2 * 9 + MAX_BODY_TILES, "{tiles} tiles");
        // The nearest crates got theirs first.
        assert!(tile_under(&mut app, 0.0, 500.0));
        // While nothing moves, nothing is built again.
        let built = app.world.resource::<TerrainColliders>().built;
        app.update_n(30);
        let state = app.world.resource::<TerrainColliders>();
        assert!(state.is_settled());
        assert_eq!(state.built, built, "tiles were rebuilt while nothing moved");
    }

    #[test]
    fn tiles_build_off_the_frame() {
        let (mut app, _) = app_with_planet();
        // The first frame builds only the tile under the focus; the rest of the block is
        // still on its way.
        let t = std::time::Instant::now();
        app.update();
        let first = t.elapsed();
        let state = app.world.resource::<TerrainColliders>();
        assert_eq!(state.built, 1);
        assert!(!state.is_settled());
        // Frames go on while the others build: three of them take less than building one tile.
        let t = std::time::Instant::now();
        app.update_n(3);
        assert!(t.elapsed() < first, "frames waited for tiles");
        settle(&mut app);
        assert_eq!(app.world.resource::<TerrainColliders>().len(), 9);
    }
}
