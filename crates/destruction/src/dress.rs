//! Detailed looks for gameplay entities and set dressing. A [`PropModel`] on an entity (a
//! building piece, a crate that can be shot apart) becomes child meshes from
//! [`crate::props`], an [`ArchModel`] fortress masonry from [`crate::architecture`], both
//! drawn in the destruction surface materials, and a [`GravelPad`] a gravel ground. Meshes
//! are built once per kind and seed and shared, so a demo that rebuilds its tower every loop
//! reuses them.

use std::collections::HashMap;

use engine_assets::{Assets, Handle};
use engine_core::math::noise::perlin2;
use engine_core::prelude::*;
use engine_render::{MaterialRef, Mesh, MeshRef, MeshRenderer};
use engine_world::physics::{Collider, ColliderShape, PhysicsBackend};
use engine_world::{Climbable, SurfaceMaterials};

use crate::architecture::{self, TowerTop};
use crate::assets::DestructionAssets;
use crate::debris::Debris;
use crate::material::{DestructibleMaterial, DestructionMaterials};
use crate::meshing::MeshBuilder;
use crate::props::PropKind;
use crate::structure::{Fracturable, StructuralPiece};

/// Draw this entity as a detailed prop (centered on the entity, like the prop's own origin).
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct PropModel {
    pub kind: PropKind,
    pub seed: u64,
    /// Linear RGB scale on the prop's colors: white as built, below it weathered or grimy.
    pub tint: [f32; 3],
}

impl PropModel {
    pub fn new(kind: PropKind, seed: u64) -> Self {
        Self {
            kind,
            seed,
            tint: [1.0; 3],
        }
    }

    /// Tint the prop's colors (see [`PropModel::tint`]).
    pub fn with_tint(mut self, tint: [f32; 3]) -> Self {
        self.tint = tint;
        self
    }
}

/// Draw this entity as fortress architecture (its base centered on the entity, y up).
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct ArchModel {
    pub kind: ArchKind,
    pub seed: u64,
}

impl ArchModel {
    pub fn new(kind: ArchKind, seed: u64) -> Self {
        Self { kind, seed }
    }
}

/// A piece of fortress masonry (see [`crate::architecture`]). Sizes are meters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ArchKind {
    /// Along X, the outer face toward −Z.
    CurtainWall {
        length: f32,
        height: f32,
        thickness: f32,
    },
    RoundTower {
        radius: f32,
        height: f32,
        top: TowerTop,
    },
    SquareTower {
        size: f32,
        height: f32,
    },
    /// Along X, a wall already breached.
    RuinedWall {
        length: f32,
        height: f32,
    },
    /// The gate passage runs along Z.
    Gatehouse {
        width: f32,
        height: f32,
        depth: f32,
    },
}

impl ArchKind {
    pub fn build(&self, seed: u64) -> Vec<architecture::ArchPart> {
        match *self {
            ArchKind::CurtainWall {
                length,
                height,
                thickness,
            } => architecture::curtain_wall(length, height, thickness, seed),
            ArchKind::RoundTower {
                radius,
                height,
                top,
            } => architecture::round_tower_with(radius, height, top, seed),
            ArchKind::SquareTower { size, height } => {
                architecture::square_tower(size, height, seed)
            }
            ArchKind::RuinedWall { length, height } => {
                architecture::ruined_wall(length, height, seed)
            }
            ArchKind::Gatehouse {
                width,
                height,
                depth,
            } => architecture::gatehouse(width, height, depth, seed),
        }
    }
}

/// A gravel ground `size` meters (X, Z) around the entity, a few centimeters thick with
/// rounded edges.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct GravelPad {
    pub size: [f32; 2],
}

/// A mesh child a [`PropModel`], [`ArchModel`] or [`GravelPad`] added.
#[derive(Component, Debug, Clone, Copy)]
pub struct PropMesh;

/// The [`PropModel`], [`ArchModel`] or [`GravelPad`] already has its children.
#[derive(Component, Debug, Clone, Copy)]
pub struct PropBuilt;

/// Prop meshes built so far, by kind and seed, and (with a physics module) the collision
/// meshes of set dressing.
#[derive(Resource, Debug, Default)]
pub struct PropMeshes {
    meshes: HashMap<String, Vec<(Handle<Mesh>, DestructibleMaterial)>>,
    solids: HashMap<String, (ColliderShape, DestructibleMaterial)>,
}

/// One triangle mesh of all `parts` (in the model's own space), and the material most of it
/// is made of.
fn solid_of(
    parts: &[(DestructibleMaterial, MeshBuilder)],
) -> (ColliderShape, DestructibleMaterial) {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for (_, mb) in parts {
        let base = vertices.len() as u32;
        vertices.extend(mb.vertices.iter().map(|v| Vec3::from(v.position)));
        indices.extend(
            mb.indices
                .as_chunks::<3>()
                .0
                .iter()
                .map(|t| [base + t[0], base + t[1], base + t[2]]),
        );
    }
    let material = parts
        .iter()
        .max_by_key(|(_, mb)| mb.indices.len())
        .map_or(DestructibleMaterial::Stone, |(m, _)| *m);
    (ColliderShape::TriMesh { vertices, indices }, material)
}

/// Give each new [`PropModel`], [`ArchModel`] and [`GravelPad`] its mesh children (Update).
/// With a physics module, fortress masonry and set-dressing props (a [`PropModel`] on nothing
/// that breaks or has a collider of its own) are solid too: a static triangle mesh of the
/// model, so the walls stop players and they can walk along the wall walks.
#[allow(clippy::too_many_arguments)]
pub fn build_prop_models_system(
    mut commands: Commands,
    assets: Option<Res<DestructionAssets>>,
    table: Res<DestructionMaterials>,
    surfaces: Res<SurfaceMaterials>,
    backend: Option<Res<PhysicsBackend>>,
    meshes: Option<ResMut<Assets<Mesh>>>,
    mut cache: ResMut<PropMeshes>,
    props: Query<(Entity, &PropModel), Without<PropBuilt>>,
    arches: Query<(Entity, &ArchModel), Without<PropBuilt>>,
    pads: Query<(Entity, &GravelPad), Without<PropBuilt>>,
    gameplay: Query<
        (),
        Or<(
            With<StructuralPiece>,
            With<Fracturable>,
            With<Debris>,
            With<Collider>,
            With<crate::door::Door>,
        )>,
    >,
    climbables: Query<(), With<Climbable>>,
) {
    let (Some(assets), Some(mut meshes)) = (assets, meshes) else {
        return;
    };
    let physics = backend.is_some();
    let mut dress =
        |e: Entity,
         key: String,
         solid: bool,
         grit: bool,
         parts: &dyn Fn() -> Vec<(DestructibleMaterial, MeshBuilder)>| {
            // A ladder is climbed through its own box: a solid one pushes the climber off the
            // climb line. As a sensor it keeps its surface, so the rungs still ring as steel.
            let sensor = climbables.contains(e);
            let solid = solid && physics;
            let need_meshes = !cache.meshes.contains_key(&key);
            if need_meshes || (solid && !cache.solids.contains_key(&key)) {
                let fresh = parts();
                if solid {
                    cache.solids.insert(key.clone(), solid_of(&fresh));
                }
                if need_meshes {
                    let built = fresh
                        .into_iter()
                        .map(|(material, mut mb)| {
                            // Vertex colors are near white (the texture brings the color); without
                            // textures they take the material's own color.
                            if !assets.textured {
                                let c = table.get(material).exterior;
                                for v in &mut mb.vertices {
                                    for (k, ch) in c.iter().enumerate() {
                                        v.color[k] *= ch;
                                    }
                                }
                            }
                            (meshes.add(mb.build()), material)
                        })
                        .collect();
                    cache.meshes.insert(key.clone(), built);
                }
            }
            for (mesh, material) in &cache.meshes[&key] {
                let surface = if grit && *material == DestructibleMaterial::Dirt {
                    assets.gravel
                } else {
                    assets.surface(*material)
                };
                spawn_part(&mut commands, e, *mesh, surface);
            }
            let mut ec = commands.entity(e);
            if solid && let Some((shape, material)) = cache.solids.get(&key) {
                let props = table.get(*material);
                ec.insert(Collider {
                    shape: shape.clone(),
                    surface: surfaces.id_or_default(&props.surface),
                    density: props.density,
                    sensor,
                    ..Collider::default()
                });
            }
            ec.insert(PropBuilt);
        };
    for (e, model) in &props {
        let solid = !gameplay.contains(e);
        let key = if model.tint == [1.0; 3] {
            format!("{:?}#{}", model.kind, model.seed)
        } else {
            format!("{:?}#{}~{:?}", model.kind, model.seed, model.tint)
        };
        dress(e, key, solid, false, &|| {
            model
                .kind
                .build(model.seed)
                .into_iter()
                .map(|mut p| {
                    for v in &mut p.mesh.vertices {
                        for (c, t) in v.color.iter_mut().zip(model.tint) {
                            *c *= t;
                        }
                    }
                    (p.material, p.mesh)
                })
                .collect()
        });
    }
    // The masonry's earth is the spoil of a breach: stone grit and rubble, drawn as gravel.
    for (e, model) in &arches {
        let solid = !gameplay.contains(e);
        dress(
            e,
            format!("{:?}#{}", model.kind, model.seed),
            solid,
            true,
            &|| {
                model
                    .kind
                    .build(model.seed)
                    .into_iter()
                    .map(|p| (p.material, p.mesh))
                    .collect()
            },
        );
    }
    for (e, pad) in &pads {
        let key = format!("{pad:?}");
        let mesh = match cache.meshes.get(&key) {
            Some(parts) => parts[0].0,
            None => {
                let mesh = meshes.add(gravel_pad(pad.size).build());
                cache
                    .meshes
                    .insert(key, vec![(mesh, DestructibleMaterial::Dirt)]);
                mesh
            }
        };
        spawn_part(&mut commands, e, mesh, assets.gravel);
        commands.entity(e).insert(PropBuilt);
    }
}

fn spawn_part(
    commands: &mut Commands,
    parent: Entity,
    mesh: Handle<Mesh>,
    material: Handle<engine_render::StandardMaterial>,
) {
    commands.spawn((
        Transform::default(),
        MeshRenderer::new(MeshRef::Handle(mesh), MaterialRef::Handle(material)),
        PropMesh,
        ChildOf(parent),
    ));
}

/// Height of a [`GravelPad`]'s top over its origin, m.
pub const GRAVEL_TOP: f32 = 0.06;

/// A gravel ground: a gently uneven top on a half-meter grid, its rim rounding down into the
/// ground, darker where wheels and feet have packed it and along the rim.
pub fn gravel_pad(size: [f32; 2]) -> MeshBuilder {
    let (hx, hz) = (size[0] * 0.5, size[1] * 0.5);
    const STEP: f32 = 0.5;
    const RIM: f32 = 0.35;
    const TOP: f32 = GRAVEL_TOP;
    let (nx, nz) = (
        ((size[0] + 2.0 * RIM) / STEP).ceil() as usize,
        ((size[1] + 2.0 * RIM) / STEP).ceil() as usize,
    );
    let at = |i: usize, k: usize| -> Vec3 {
        let x = -hx - RIM + (size[0] + 2.0 * RIM) * i as f32 / nx as f32;
        let z = -hz - RIM + (size[1] + 2.0 * RIM) * k as f32 / nz as f32;
        // Distance inside the pad's edge (negative on the rim).
        let inside = (hx - x.abs()).min(hz - z.abs());
        let rim = ((inside + RIM) / RIM).clamp(0.0, 1.0);
        let lift = TOP * (rim * rim * (3.0 - 2.0 * rim));
        let bumps = 0.012 * perlin2(x as f64 * 1.7, z as f64 * 1.7, 0x6EA7);
        Vec3::new(x, lift + bumps * rim - 0.02 * (1.0 - rim), z)
    };
    let mut mb = MeshBuilder::new();
    for k in 0..=nz {
        for i in 0..=nx {
            let p = at(i, k);
            let dx = at((i + 1).min(nx), k) - at(i.saturating_sub(1), k);
            let dz = at(i, (k + 1).min(nz)) - at(i, k.saturating_sub(1));
            let n = dz.cross(dx).normalize_or(Vec3::Y);
            let inside = (hx - p.x.abs()).min(hz - p.z.abs());
            let edge = (inside / 1.5).clamp(0.0, 1.0);
            // Packed lanes along the range and mottling.
            let lane = (-(p.z + 1.2).powi(2) / 1.2).exp() * 0.12;
            let mottle = 0.08 * perlin2(p.x as f64 * 0.35, p.z as f64 * 0.35, 0x6EA8);
            let shade = (0.82 + 0.18 * edge) * (1.0 - lane) * (1.0 + mottle);
            mb.vertex(p, n, [shade; 3]);
        }
    }
    let row = nx as u32 + 1;
    for k in 0..nz as u32 {
        for i in 0..nx as u32 {
            let a = k * row + i;
            mb.triangle(a, a + row, a + 1);
            mb.triangle(a + 1, a + row, a + row + 1);
        }
    }
    mb
}
