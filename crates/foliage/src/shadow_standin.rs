//! Near trees cast their shadows from their middle level of detail. A tree's leaf-by-leaf
//! crown (150k triangles for an oak) and near level (40k) cost the shadow maps as much again
//! in every cascade they reach, for a shadow no one can tell from the middle level's flat cards
//! (9k): the same cards in the same places, cut out by the same leaf atlas. So a part showing a
//! near or leaf-by-leaf mesh casts no shadow itself; a child drawn only into the shadow maps
//! ([`ShadowOnly`]) casts its middle-level mesh instead. Unity's `ShadowCastingMode.ShadowsOnly`
//! stand-ins and Horizon Zero Dawn's separate vegetation shadow meshes (one fixed level per
//! cascade) do the same. A side effect: the shadow no longer changes as a tree switches between
//! leaf-by-leaf, near and middle levels. Research: engine-design/foliage-research.md, 4.1 and 4.4.

use std::collections::HashMap;

use crate::assets::{FoliageVisuals, SpeciesVisual};
use crate::visuals::PlantPart;
use engine_assets::Handle;
use engine_core::prelude::*;
use engine_render::{MaterialRef, Mesh, MeshRef, MeshRenderer, ShadowOnly, StandardMaterial};

/// Whether a plant part casts a shadow, its own or its stand-in's (without one: it does).
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct CastsShadow(pub bool);

/// A part's shadow stand-in: a child entity drawn into the shadow maps only.
#[derive(Component, Debug, Clone, Copy)]
pub struct StandIn(pub Entity);

/// Marks a shadow stand-in entity.
#[derive(Component, Debug, Clone, Copy)]
pub struct ShadowStandIn;

/// Which meshes cast through a stand-in, and with what: near and leaf-by-leaf meshes to their
/// middle-level ones (by mesh handle index), and the leaf-by-leaf materials to the near leaf
/// materials the middle level's cards are drawn with.
#[derive(Debug, Default, Clone)]
pub struct StandIns {
    meshes: HashMap<u32, Handle<Mesh>>,
    materials: HashMap<u32, Handle<StandardMaterial>>,
}

impl StandIns {
    /// Add a species' stand-ins.
    pub fn add(&mut self, vis: &SpeciesVisual) {
        for v in &vis.variants {
            let mut map = |from: Option<Handle<Mesh>>, to: Option<Handle<Mesh>>| {
                if let (Some(from), Some(to)) = (from, to) {
                    self.meshes.insert(from.index(), to);
                }
            };
            map(v.wood, v.wood_mid.or(v.wood_lod));
            map(v.foliage, v.foliage_mid);
            map(v.foliage_hero, v.foliage_mid);
            for (from, to) in v.foliage_thin.iter().zip(&v.foliage_mid_thin) {
                map(Some(*from), Some(*to));
            }
        }
        if let Some(hero) = &vis.leaves_hero {
            for (from, to) in hero.iter().zip(&vis.leaves) {
                self.materials.insert(from.index(), *to);
            }
        }
    }

    /// What a part drawing `mesh` with `material` casts its shadow with, if not itself.
    pub fn get(
        &self,
        mesh: &MeshRef,
        material: &MaterialRef,
    ) -> Option<(Handle<Mesh>, Handle<StandardMaterial>)> {
        let (MeshRef::Handle(mesh), MaterialRef::Handle(material)) = (mesh, material) else {
            return None;
        };
        let to = *self.meshes.get(&mesh.index())?;
        let material = self
            .materials
            .get(&material.index())
            .copied()
            .unwrap_or(*material);
        Some((to, material))
    }
}

/// Give parts showing a near or leaf-by-leaf mesh a shadow stand-in, and hide it again when they
/// switch to a level that casts itself.
#[allow(clippy::type_complexity)]
pub fn sync_shadow_standins(
    mut commands: Commands,
    visuals: Res<FoliageVisuals>,
    mut parts: Query<
        (
            Entity,
            &mut MeshRenderer,
            Option<&CastsShadow>,
            Option<&StandIn>,
        ),
        (
            With<PlantPart>,
            Or<(Changed<MeshRenderer>, Changed<CastsShadow>)>,
        ),
    >,
    mut standins: Query<&mut MeshRenderer, (With<ShadowStandIn>, Without<PlantPart>)>,
) {
    for (entity, mut r, wants, standin) in &mut parts {
        let wants = wants.is_none_or(|w| w.0) && r.visible;
        let stand = wants
            .then(|| visuals.standins.get(&r.mesh, &r.material))
            .flatten();
        let own = wants && stand.is_none();
        if r.cast_shadows != own {
            r.cast_shadows = own;
        }
        match (stand, standin) {
            (Some((mesh, material)), Some(s)) => {
                if let Ok(mut s) = standins.get_mut(s.0) {
                    let (mesh, material) = (MeshRef::Handle(mesh), MaterialRef::Handle(material));
                    if s.mesh != mesh || s.material != material || !s.visible {
                        s.mesh = mesh;
                        s.material = material;
                        s.visible = true;
                    }
                }
            }
            (Some((mesh, material)), None) => {
                let mut s = MeshRenderer::new(
                    engine_render::Shape::Cuboid { size: [0.01; 3] },
                    StandardMaterial::default(),
                );
                s.mesh = MeshRef::Handle(mesh);
                s.material = MaterialRef::Handle(material);
                let s = commands
                    .spawn((
                        Transform::default(),
                        ChildOf(entity),
                        s,
                        ShadowOnly,
                        ShadowStandIn,
                    ))
                    .id();
                commands.entity(entity).insert(StandIn(s));
            }
            (None, Some(s)) => {
                if let Ok(mut s) = standins.get_mut(s.0)
                    && s.visible
                {
                    s.visible = false;
                }
            }
            (None, None) => {}
        }
    }
}
