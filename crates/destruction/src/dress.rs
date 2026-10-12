//! Detailed looks for gameplay entities. A [`PropModel`] on an entity (a building piece, a
//! crate that can be shot apart) becomes child meshes from [`crate::props`], drawn in the
//! destruction surface materials. Meshes are built once per kind and seed and shared, so a
//! demo that rebuilds its tower every loop reuses them.

use std::collections::HashMap;

use engine_assets::{Assets, Handle};
use engine_core::prelude::*;
use engine_render::{MaterialRef, Mesh, MeshRef, MeshRenderer};

use crate::assets::DestructionAssets;
use crate::material::{DestructibleMaterial, DestructionMaterials};
use crate::props::PropKind;

/// Draw this entity as a detailed prop (centered on the entity, like the prop's own origin).
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct PropModel {
    pub kind: PropKind,
    pub seed: u64,
}

impl PropModel {
    pub fn new(kind: PropKind, seed: u64) -> Self {
        Self { kind, seed }
    }
}

/// A mesh child a [`PropModel`] added.
#[derive(Component, Debug, Clone, Copy)]
pub struct PropMesh;

/// The [`PropModel`] already has its children.
#[derive(Component, Debug, Clone, Copy)]
pub struct PropBuilt;

/// Prop meshes built so far, by kind and seed.
#[derive(Resource, Debug, Default)]
pub struct PropMeshes(HashMap<String, Vec<(Handle<Mesh>, DestructibleMaterial)>>);

/// Give each new [`PropModel`] its mesh children (Update).
pub fn build_prop_models_system(
    mut commands: Commands,
    assets: Option<Res<DestructionAssets>>,
    table: Res<DestructionMaterials>,
    meshes: Option<ResMut<Assets<Mesh>>>,
    mut cache: ResMut<PropMeshes>,
    models: Query<(Entity, &PropModel), Without<PropBuilt>>,
) {
    let (Some(assets), Some(mut meshes)) = (assets, meshes) else {
        return;
    };
    for (e, model) in &models {
        let key = format!("{:?}#{}", model.kind, model.seed);
        let parts = cache.0.entry(key).or_insert_with(|| {
            model
                .kind
                .build(model.seed)
                .into_iter()
                .map(|part| {
                    let mut mb = part.mesh;
                    // Prop vertex colors are near white (the texture brings the color); without
                    // textures they take the material's own color.
                    if !assets.textured {
                        let c = table.get(part.material).exterior;
                        for v in &mut mb.vertices {
                            for (k, ch) in c.iter().enumerate() {
                                v.color[k] *= ch;
                            }
                        }
                    }
                    (meshes.add(mb.build()), part.material)
                })
                .collect()
        });
        for (mesh, material) in parts.iter() {
            commands.spawn((
                Transform::default(),
                MeshRenderer::new(
                    MeshRef::Handle(*mesh),
                    MaterialRef::Handle(assets.surface(*material)),
                ),
                PropMesh,
                ChildOf(e),
            ));
        }
        commands.entity(e).insert(PropBuilt);
    }
}
