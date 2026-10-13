//! Keeps the runtime state, meshes and colliders of walls and voxel volumes in step with
//! their components: (re)builds state when a component is added or changed, and remeshes only
//! the tiles and chunks that damage touched. Standing building pieces get colliders too.

use std::time::Instant;

use bevy_ecs::change_detection::Tick;
use engine_assets::{Assets, Handle};
use engine_core::math::{Quat, Vec2, Vec3};
use engine_core::prelude::*;
use engine_render::{MaterialRef, Mesh, MeshRef, MeshRenderer};
use engine_world::SurfaceMaterials;
use engine_world::physics::{Collider, ColliderShape, PhysicsBackend};

use crate::DestructionStats;
use crate::assets::DestructionAssets;
use crate::build::BuiltPiece;
use crate::debris::Debris;
use crate::material::DestructionMaterials;
use crate::meshing::MeshBuilder;
use crate::structure::{Fracturable, FractureShape, StructuralPiece};
use crate::voxel::{VoxelLook, VoxelState, VoxelVolume, mesh_chunk_layers};
use crate::wall::{DestructibleWall, TILE, WallLook, WallState, greedy_rects, mesh_cells};

/// One mesh tile of a wall (child entity).
#[derive(Component, Debug, Clone, Copy)]
pub struct WallTile {
    pub wall: Entity,
    pub tile: usize,
    pub mesh: Handle<Mesh>,
}

/// One mesh chunk of a voxel volume (child entity, with the chunk's collider). It draws
/// through one child per surface layer (rock, soil, grass), each with its own material.
#[derive(Component, Debug, Clone)]
pub struct VoxelChunk {
    pub volume: Entity,
    pub chunk: usize,
    /// (layer, drawing entity, mesh); see [`crate::voxel::mesh_chunk_layers`].
    pub layers: Vec<(u8, Entity, Handle<Mesh>)>,
}

/// One surface layer of a [`VoxelChunk`] (its drawing child).
#[derive(Component, Debug, Clone, Copy)]
pub struct VoxelLayer;

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
                        for (_, _, mesh) in &chunk.layers {
                            m.remove(*mesh);
                        }
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

/// How far a standing piece's collider sits inside its box on every side, m: pieces snapped
/// face to face don't start a fall wedged into the neighbors that still stand.
pub const PIECE_SKIN_M: f32 = 0.015;

/// Half size of a standing piece's collider for a piece box of half size `half`.
pub fn solid_half(half: Vec3) -> Vec3 {
    (half - Vec3::splat(PIECE_SKIN_M)).max(half * 0.5)
}

/// Make standing [`StructuralPiece`]s solid when a physics module is installed: a static
/// collider in the piece's box (a gable's triangle) with its material's surface. Players walk
/// on floors and decks and are stopped by walls, vehicles crash into them, and debris lands on
/// them. A piece that gives way becomes a debris body with its own hull. [`Fracturable`]
/// props (crates, columns) get their box or column until they break.
pub fn piece_colliders_system(
    mut commands: Commands,
    backend: Option<Res<PhysicsBackend>>,
    table: Res<DestructionMaterials>,
    surfaces: Res<SurfaceMaterials>,
    props: Query<(Entity, &Fracturable), (Without<Collider>, Without<Debris>)>,
    pieces: Query<
        (
            Entity,
            &StructuralPiece,
            &Transform,
            Option<&GlobalTransform>,
            Has<ChildOf>,
            Option<&BuiltPiece>,
        ),
        (Without<Collider>, Without<Debris>),
    >,
) {
    if backend.is_none() {
        return;
    }
    for (e, prop) in &props {
        let props = table.get(prop.material);
        commands.entity(e).insert(Collider {
            shape: match prop.shape {
                FractureShape::Box { half_extents } => ColliderShape::Cuboid { half_extents },
                FractureShape::Column {
                    radius,
                    half_height,
                    ..
                } => ColliderShape::Cylinder {
                    radius,
                    half_height,
                },
            },
            surface: surfaces.id_or_default(&props.surface),
            density: props.density,
            ..Collider::default()
        });
    }
    for (e, piece, t, gt, child, built) in &pieces {
        // The backend scales shapes by the entity's world scale; the piece's box is in meters.
        let scale = match (child, gt) {
            (true, Some(gt)) => gt.scale(),
            _ => t.scale,
        }
        .max(Vec3::splat(1e-3));
        let half = solid_half(piece.half_extents) / scale;
        let props = table.get(piece.material);
        commands.entity(e).insert(Collider {
            shape: built.map_or(ColliderShape::Cuboid { half_extents: half }, |b| {
                b.kind.solid(half)
            }),
            surface: surfaces.id_or_default(&props.surface),
            density: props.density,
            ..Collider::default()
        });
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
        let look = WallLook::new(wall, &table).with_texture(assets.textured);
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
    mut renderers: Query<&mut MeshRenderer, With<VoxelLayer>>,
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
        let look = VoxelLook::new(volume, &table).with_layers(assets.textured);
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
            let layers = mesh_chunk_layers(&state, &look, [cx, cy, cz]);
            let empty = layers.iter().all(|(_, m)| m.is_empty());
            let collider = (backend.is_some() && !empty).then(|| {
                let mut all = MeshBuilder::new();
                for (_, m) in &layers {
                    all.append(m);
                }
                Collider {
                    shape: ColliderShape::TriMesh {
                        vertices: all
                            .vertices
                            .iter()
                            .map(|v| Vec3::from(v.position))
                            .collect(),
                        indices: all
                            .indices
                            .as_chunks::<3>()
                            .0
                            .iter()
                            .map(|t| [t[0], t[1], t[2]])
                            .collect(),
                    },
                    ..Collider::default()
                }
            });
            let existing =
                state.chunk_entities[ci].and_then(|c| chunks.get(c).ok().map(|v| (c, v)));
            let (ce, mut chunk) = match existing {
                Some((ce, vc)) => (ce, vc.clone()),
                None => {
                    if empty {
                        continue;
                    }
                    let chunk = VoxelChunk {
                        volume: e,
                        chunk: ci,
                        layers: Vec::new(),
                    };
                    let ce = commands
                        .spawn((Transform::default(), chunk.clone(), ChildOf(e)))
                        .id();
                    state.chunk_entities[ci] = Some(ce);
                    (ce, chunk)
                }
            };
            // Each layer's drawing child: shown while the layer has triangles, spawned the
            // first time it does.
            for &(tag, le, _) in &chunk.layers {
                let present = layers.iter().any(|(t, m)| *t == tag && !m.is_empty());
                if let Ok(mut r) = renderers.get_mut(le)
                    && r.visible != present
                {
                    r.visible = present;
                }
            }
            for (tag, mb) in layers {
                if mb.is_empty() {
                    continue;
                }
                match chunk.layers.iter().find(|l| l.0 == tag) {
                    Some(&(_, _, h)) => {
                        meshes.set(h, mb.build());
                    }
                    None => {
                        let h = meshes.add(mb.build());
                        let le = commands
                            .spawn((
                                Transform::default(),
                                MeshRenderer::new(
                                    MeshRef::Handle(h),
                                    MaterialRef::Handle(assets.layer_material(tag)),
                                ),
                                VoxelLayer,
                                ChildOf(ce),
                            ))
                            .id();
                        chunk.layers.push((tag, le, h));
                    }
                }
            }
            let mut ec = commands.entity(ce);
            ec.insert(chunk);
            match collider {
                Some(c) => {
                    ec.insert(c);
                }
                None => {
                    ec.remove::<Collider>();
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
