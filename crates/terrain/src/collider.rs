//! Physics colliders for terrain: heightfield tiles around the simulation focus.
//!
//! Tiles are quadtree patches of about [`TerrainSettings::collider_tile_m`] on the terrain,
//! so the tiling is stable anywhere on a planet. Each tile is a static entity with a
//! [`Collider`](engine_world::physics::Collider) `Heightfield` in the tile's tangent frame
//! (local +Y = the planet's up at the tile center). Heights are at full detail, including
//! edits. Tiles are rebuilt when an edit touches them. Tiles cover the 3 × 3 block around
//! every focus point and the tile under every dynamic body.
//!
//! Heightfield layout: `heights[row * cols + col]`, rows along local Z from −size/2 to
//! +size/2, columns along local X from −size/2 to +size/2, heights in meters above the tile
//! origin.

use std::collections::{HashMap, HashSet};

use engine_core::math::{DMat3, DQuat, DVec3};
use engine_core::prelude::*;
use engine_world::physics::{BodyKind, Collider, ColliderShape, RigidBody};
use engine_world::{FocusPoints, SurfaceMaterials};

use crate::cube::NodeKey;
use crate::palette::SurfaceIds;
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
}

impl TerrainColliders {
    pub fn len(&self) -> usize {
        self.tiles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }
}

/// Level whose patches are closest to `tile_m` across.
fn tile_level(t: &TerrainRuntime, tile_m: f64) -> u8 {
    let root = t.source.map.root_size();
    (root / tile_m.max(1.0)).log2().round().clamp(0.0, 24.0) as u8
}

/// Build the heightfield for one tile: (world transform, collider).
pub fn build_tile(
    t: &TerrainRuntime,
    key: NodeKey,
    samples: u32,
    ids: &SurfaceIds,
) -> (Transform, Collider) {
    let map = &t.source.map;
    let (lo, size) = key.face_rect();
    let center_ab = lo + size * 0.5;
    let (base_c, up, dir_c) = map.base(key.face, center_ab.x, center_ab.y);
    let center = t.source.sample(base_c, up, dir_c, 0.0);
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
            let s = t.source.sample(base, u, dir, 0.0);
            let p = base + u * s.height - origin;
            heights[(row * n + col) as usize] = p.dot(up) as f32;
            extent_x = extent_x.max(p.dot(x_axis).abs());
            extent_z = extent_z.max(p.dot(z_axis).abs());
        }
    }
    let surface = t.config.palette.classify(
        &center,
        0.0,
        center.height < 0.0 && t.source.shape.ocean,
        ids,
    );
    let rot_local = DQuat::from_mat3(&DMat3::from_cols(x_axis, up, z_axis));
    let world_rot = t.rotation * rot_local;
    let transform = Transform::from_translation(t.position + t.rotation * origin)
        .with_rotation(world_rot.as_quat());
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
    (transform, collider)
}

/// Most tiles kept under dynamic bodies away from every focus point, nearest first.
pub const MAX_BODY_TILES: usize = 64;
/// Most tiles built in one frame; the first is always built, the rest while the frame's
/// building stays under [`BUILD_BUDGET_MS`].
const MAX_BUILDS_PER_FRAME: usize = 8;
const BUILD_BUDGET_MS: f64 = 2.0;

/// Keep collider tiles on every terrain that wants colliders: the 3 × 3 block around every
/// focus point, and the tile under every dynamic body (awake or asleep, so a crate left
/// behind still has ground when something wakes it), up to [`MAX_BODY_TILES`] of those.
pub fn update_colliders(
    mut commands: Commands,
    terrains: Res<Terrains>,
    settings: Res<TerrainSettings>,
    focus: Res<FocusPoints>,
    materials: Res<SurfaceMaterials>,
    bodies: Query<(&Transform, &RigidBody)>,
    mut state: ResMut<TerrainColliders>,
) {
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
    // Build a few queued tiles (one at least, more while the frame's budget lasts).
    let started = std::time::Instant::now();
    let ids = SurfaceIds::new(&materials);
    for built in 0..MAX_BUILDS_PER_FRAME {
        if built > 0 && started.elapsed().as_secs_f64() * 1000.0 > BUILD_BUDGET_MS {
            break;
        }
        if state.queue.is_empty() {
            break;
        }
        let (e, key) = state.queue.remove(0);
        let Some(t) = terrains.entries.get(&e) else {
            continue;
        };
        let (transform, collider) = build_tile(t, key, settings.collider_samples, &ids);
        if let Some(old) = state.tiles.get(&(e, key)) {
            commands.entity(*old).insert((transform, collider));
        } else {
            let id = commands
                .spawn((transform, collider, TerrainColliderTile { terrain: e, key }))
                .id();
            state.tiles.insert((e, key), id);
        }
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
        app.update_n(20);
        assert!(tile_under(&mut app, 0.0, 0.0), "no ground at the focus");
        assert!(
            tile_under(&mut app, 1_000.0, 0.0),
            "no ground under the crate"
        );
        // Kinematic bodies are moved by gameplay, not dropped: they get no tile of their own.
        assert!(!tile_under(&mut app, 0.0, -1_500.0));
        // The focus flies 2 km off: the crate keeps its ground, the old block goes.
        app.world.get_mut::<Transform>(focus).unwrap().translation = DVec3::new(-2_000.0, 2.0, 0.0);
        app.update_n(20);
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
        // At least one tile a frame (more while each frame's budget lasts).
        app.update_n(2 * 9 + MAX_BODY_TILES + 5);
        assert!(
            tile_under(&mut app, 3_000.0, 0.0),
            "no ground at the second focus"
        );
        let tiles = app.world.resource::<TerrainColliders>().len();
        assert_eq!(tiles, 2 * 9 + MAX_BODY_TILES, "{tiles} tiles");
        // The nearest crates got theirs first.
        assert!(tile_under(&mut app, 0.0, 500.0));
    }
}
