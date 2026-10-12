//! Physics colliders for terrain: heightfield tiles around the simulation focus.
//!
//! Tiles are quadtree patches of about [`TerrainSettings::collider_tile_m`] on the terrain,
//! so the tiling is stable anywhere on a planet. Each tile is a static entity with a
//! [`Collider`](engine_world::physics::Collider) `Heightfield` in the tile's tangent frame
//! (local +Y = the planet's up at the tile center). Heights are at full detail, including
//! edits. Tiles are rebuilt when an edit touches them, at most one per frame.
//!
//! Heightfield layout: `heights[row * cols + col]`, rows along local Z from −size/2 to
//! +size/2, columns along local X from −size/2 to +size/2, heights in meters above the tile
//! origin.

use std::collections::HashMap;

use engine_core::math::{DMat3, DQuat};
use engine_core::prelude::*;
use engine_world::physics::{Collider, ColliderShape};
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

/// Keep collider tiles around the primary focus on every terrain that wants colliders.
pub fn update_colliders(
    mut commands: Commands,
    terrains: Res<Terrains>,
    settings: Res<TerrainSettings>,
    focus: Res<FocusPoints>,
    materials: Res<SurfaceMaterials>,
    mut state: ResMut<TerrainColliders>,
) {
    if focus.points.is_empty() {
        return;
    }
    let p = focus.primary();
    let mut wanted: Vec<(Entity, NodeKey)> = Vec::new();
    for (&e, t) in &terrains.entries {
        if !t.config.colliders {
            continue;
        }
        // Only near the surface (within 2 km).
        if t.altitude_hint(p).abs() > 2_000.0 + t.config.height_range().1 {
            continue;
        }
        let level = tile_level(t, settings.collider_tile_m);
        let local = t.to_local(p);
        let Some((_, up, _)) = t.source.map.locate(local) else {
            continue;
        };
        let size = t.source.map.node_size(level);
        let side = up.any_orthonormal_vector();
        let fwd = up.cross(side);
        for dz in -1..=1 {
            for dx in -1..=1 {
                let q = local + (side * dx as f64 + fwd * dz as f64) * size;
                if let Some(k) = t.source.map.node_at(q, level)
                    && !wanted.contains(&(e, k))
                {
                    wanted.push((e, k));
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
    // Remove tiles no longer wanted.
    let stale: Vec<_> = state
        .tiles
        .keys()
        .filter(|k| !wanted.contains(k))
        .copied()
        .collect();
    for k in stale {
        if let Some(entity) = state.tiles.remove(&k) {
            commands.entity(entity).despawn();
        }
    }
    for k in &wanted {
        if !state.tiles.contains_key(k) && !state.queue.contains(k) {
            state.queue.push(*k);
        }
    }
    // Build the nearest queued tile (one per frame keeps hitches small).
    state.queue.retain(|k| wanted.contains(k));
    state.queue.dedup();
    if let Some((e, key)) = state.queue.first().copied() {
        state.queue.remove(0);
        let Some(t) = terrains.entries.get(&e) else {
            return;
        };
        let ids = SurfaceIds::new(&materials);
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
}
