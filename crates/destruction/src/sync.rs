//! Keeps the runtime state, meshes and colliders of walls and voxel volumes in step with
//! their components: (re)builds state when a component is added or changed, and remeshes only
//! the tiles and chunks that damage touched.

use std::time::Instant;

use bevy_ecs::change_detection::Tick;
use engine_assets::{Assets, Handle};
use engine_core::math::{Quat, Vec2, Vec3};
use engine_core::prelude::*;
use engine_render::{MaterialRef, Mesh, MeshRef, MeshRenderer};
use engine_world::physics::{Collider, ColliderShape, PhysicsBackend};

use crate::DestructionStats;
use crate::assets::DestructionAssets;
use crate::material::DestructionMaterials;
use crate::voxel::{VoxelLook, VoxelState, VoxelVolume, mesh_chunk};
use crate::wall::{DestructibleWall, TILE, WallLook, WallState, greedy_rects, mesh_cells};

/// One mesh tile of a wall (child entity).
#[derive(Component, Debug, Clone, Copy)]
pub struct WallTile {
    pub wall: Entity,
    pub tile: usize,
    pub mesh: Handle<Mesh>,
}

/// One mesh chunk of a voxel volume (child entity).
#[derive(Component, Debug, Clone, Copy)]
pub struct VoxelChunk {
    pub volume: Entity,
    pub chunk: usize,
    pub mesh: Handle<Mesh>,
}

/// When a wall's or volume's state was built from its component (change tick), so a state
/// that was built (and damaged) before this system first ran is not rebuilt.
#[derive(Component, Debug, Clone, Copy)]
pub struct BuiltAt(pub Tick);

/// (Re)create [`WallState`] for new or edited walls.
pub fn init_walls_system(
    mut commands: Commands,
    table: Res<DestructionMaterials>,
    mut q: Query<
        (
            Entity,
            Ref<DestructibleWall>,
            Option<&mut WallState>,
            Option<&BuiltAt>,
        ),
        Changed<DestructibleWall>,
    >,
    tiles: Query<&WallTile>,
    mut meshes: Option<ResMut<Assets<Mesh>>>,
) {
    for (e, wall, state, built) in &mut q {
        if built.is_some_and(|b| b.0 == wall.last_changed()) {
            continue;
        }
        commands.entity(e).insert(BuiltAt(wall.last_changed()));
        let fresh = WallState::new(&wall, &table);
        match state {
            Some(mut s) => {
                for t in s.tile_entities.iter().flatten() {
                    if let Ok(tile) = tiles.get(*t)
                        && let Some(m) = meshes.as_deref_mut()
                    {
                        m.remove(tile.mesh);
                    }
                    commands.entity(*t).despawn();
                }
                *s = fresh;
            }
            None => {
                commands.entity(e).insert(fresh);
            }
        }
    }
}

/// (Re)create [`VoxelState`] for new or edited volumes.
pub fn init_voxels_system(
    mut commands: Commands,
    table: Res<DestructionMaterials>,
    mut q: Query<
        (
            Entity,
            Ref<VoxelVolume>,
            Option<&mut VoxelState>,
            Option<&BuiltAt>,
        ),
        Changed<VoxelVolume>,
    >,
    chunks: Query<&VoxelChunk>,
    mut meshes: Option<ResMut<Assets<Mesh>>>,
) {
    for (e, volume, state, built) in &mut q {
        if built.is_some_and(|b| b.0 == volume.last_changed()) {
            continue;
        }
        commands.entity(e).insert(BuiltAt(volume.last_changed()));
        let fresh = VoxelState::new(&volume, &table);
        match state {
            Some(mut s) => {
                for c in s.chunk_entities.iter().flatten() {
                    if let Ok(chunk) = chunks.get(*c)
                        && let Some(m) = meshes.as_deref_mut()
                    {
                        m.remove(chunk.mesh);
                    }
                    commands.entity(*c).despawn();
                }
                *s = fresh;
            }
            None => {
                commands.entity(e).insert(fresh);
            }
        }
    }
}

/// Remesh dirty wall tiles and rebuild wall colliders.
#[allow(clippy::too_many_arguments)]
pub fn rebuild_walls_system(
    mut commands: Commands,
    table: Res<DestructionMaterials>,
    assets: Option<Res<DestructionAssets>>,
    meshes: Option<ResMut<Assets<Mesh>>>,
    mut walls: Query<(Entity, &DestructibleWall, &mut WallState)>,
    tiles: Query<&WallTile>,
    mut renderers: Query<&mut MeshRenderer, With<WallTile>>,
    mut diagnostics: ResMut<Diagnostics>,
    mut stats: ResMut<DestructionStats>,
) {
    let (Some(assets), Some(mut meshes)) = (assets, meshes) else {
        return;
    };
    let start = Instant::now();
    let mut rebuilt = 0;
    let mut count = 0;
    for (e, wall, mut state) in &mut walls {
        count += 1;
        if state.collider_dirty {
            state.collider_dirty = false;
            let g = &state.grid;
            let shapes: Vec<_> = greedy_rects(g)
                .into_iter()
                .map(|(i0, j0, i1, j1)| {
                    let w = (i1 - i0) as f32 * g.cell.x;
                    let h = (j1 - j0) as f32 * g.cell.y;
                    let c = g.origin
                        + Vec2::new(
                            i0 as f32 * g.cell.x + w * 0.5,
                            j0 as f32 * g.cell.y + h * 0.5,
                        );
                    (
                        Vec3::new(c.x, c.y, 0.0),
                        Quat::IDENTITY,
                        ColliderShape::Cuboid {
                            half_extents: Vec3::new(w * 0.5, h * 0.5, g.thickness * 0.5),
                        },
                    )
                })
                .collect();
            let props = table.get(wall.material);
            commands.entity(e).insert(Collider {
                shape: ColliderShape::Compound(shapes),
                density: props.density,
                ..Collider::default()
            });
        }
        if !state.tile_dirty.iter().any(|d| *d) {
            continue;
        }
        let look = WallLook::new(wall, &table);
        let material = assets.surface(wall.material);
        for tile in 0..state.tile_dirty.len() {
            if !state.tile_dirty[tile] {
                continue;
            }
            state.tile_dirty[tile] = false;
            rebuilt += 1;
            let tx = tile % state.tiles_x;
            let ty = tile / state.tiles_x;
            let rect = (
                tx * TILE,
                ty * TILE,
                ((tx + 1) * TILE).min(state.grid.nx),
                ((ty + 1) * TILE).min(state.grid.ny),
            );
            let mb = mesh_cells(&state.grid, &look, rect, Vec3::ZERO);
            let empty = mb.is_empty();
            match state.tile_entities[tile].and_then(|t| tiles.get(t).ok().map(|w| (t, *w))) {
                Some((te, wt)) => {
                    if !empty {
                        meshes.set(wt.mesh, mb.build());
                    }
                    if let Ok(mut r) = renderers.get_mut(te)
                        && r.visible == empty
                    {
                        r.visible = !empty;
                    }
                }
                None => {
                    if empty {
                        continue;
                    }
                    let h = meshes.add(mb.build());
                    let te = commands
                        .spawn((
                            Transform::default(),
                            MeshRenderer::new(MeshRef::Handle(h), MaterialRef::Handle(material)),
                            WallTile {
                                wall: e,
                                tile,
                                mesh: h,
                            },
                            ChildOf(e),
                        ))
                        .id();
                    state.tile_entities[tile] = Some(te);
                }
            }
        }
    }
    stats.walls = count;
    if rebuilt > 0 {
        diagnostics.set(
            "destruction.wall_remesh_ms",
            start.elapsed().as_secs_f64() * 1000.0,
        );
    }
}

/// Remesh dirty voxel chunks (and their triangle-mesh colliders when physics is installed).
#[allow(clippy::too_many_arguments)]
pub fn rebuild_voxels_system(
    mut commands: Commands,
    table: Res<DestructionMaterials>,
    assets: Option<Res<DestructionAssets>>,
    meshes: Option<ResMut<Assets<Mesh>>>,
    backend: Option<Res<PhysicsBackend>>,
    mut volumes: Query<(Entity, &VoxelVolume, &mut VoxelState)>,
    chunks: Query<&VoxelChunk>,
    mut renderers: Query<&mut MeshRenderer, With<VoxelChunk>>,
    mut diagnostics: ResMut<Diagnostics>,
    mut stats: ResMut<DestructionStats>,
) {
    let (Some(assets), Some(mut meshes)) = (assets, meshes) else {
        return;
    };
    let start = Instant::now();
    let mut rebuilt = 0;
    let mut count = 0;
    for (e, volume, mut state) in &mut volumes {
        count += 1;
        if !state.chunk_dirty.iter().any(|d| *d) {
            continue;
        }
        let look = VoxelLook::new(volume, &table);
        let material = assets.surface(volume.material);
        let [cx_n, cy_n, _] = state.chunks;
        for ci in 0..state.chunk_dirty.len() {
            if !state.chunk_dirty[ci] {
                continue;
            }
            state.chunk_dirty[ci] = false;
            rebuilt += 1;
            let cx = ci % cx_n;
            let cy = (ci / cx_n) % cy_n;
            let cz = ci / (cx_n * cy_n);
            let mb = mesh_chunk(&state, &look, [cx, cy, cz]);
            let empty = mb.is_empty();
            let collider = (backend.is_some() && !empty).then(|| Collider {
                shape: ColliderShape::TriMesh {
                    vertices: mb.vertices.iter().map(|v| Vec3::from(v.position)).collect(),
                    indices: mb
                        .indices
                        .as_chunks::<3>()
                        .0
                        .iter()
                        .map(|t| [t[0], t[1], t[2]])
                        .collect(),
                },
                ..Collider::default()
            });
            match state.chunk_entities[ci].and_then(|c| chunks.get(c).ok().map(|v| (c, *v))) {
                Some((ce, vc)) => {
                    if !empty {
                        meshes.set(vc.mesh, mb.build());
                    }
                    if let Ok(mut r) = renderers.get_mut(ce)
                        && r.visible == empty
                    {
                        r.visible = !empty;
                    }
                    match collider {
                        Some(c) => {
                            commands.entity(ce).insert(c);
                        }
                        None => {
                            commands.entity(ce).remove::<Collider>();
                        }
                    }
                }
                None => {
                    if empty {
                        continue;
                    }
                    let h = meshes.add(mb.build());
                    let mut ec = commands.spawn((
                        Transform::default(),
                        MeshRenderer::new(MeshRef::Handle(h), MaterialRef::Handle(material)),
                        VoxelChunk {
                            volume: e,
                            chunk: ci,
                            mesh: h,
                        },
                        ChildOf(e),
                    ));
                    if let Some(c) = collider {
                        ec.insert(c);
                    }
                    state.chunk_entities[ci] = Some(ec.id());
                }
            }
        }
    }
    stats.voxel_volumes = count;
    if rebuilt > 0 {
        diagnostics.set(
            "destruction.voxel_remesh_ms",
            start.elapsed().as_secs_f64() * 1000.0,
        );
    }
}
