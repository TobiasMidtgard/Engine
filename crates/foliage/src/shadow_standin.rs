//! Near trees cast their shadows from their middle level of detail. A tree's leaf-by-leaf
//! crown (150k triangles for an oak) and near level (40k) cost the shadow maps as much again
//! in every cascade they reach, for a shadow no one can tell from the middle level's flat cards
//! (9k): the same cards in the same places, cut out by the same leaf atlas. So a part showing a
//! near or leaf-by-leaf mesh casts no shadow itself; a child drawn only into the shadow maps
//! ([`ShadowOnly`]) casts its middle-level mesh instead. Unity's `ShadowCastingMode.ShadowsOnly`
//! stand-ins and Horizon Zero Dawn's separate vegetation shadow meshes (one fixed level per
//! cascade) do the same. A side effect: the shadow no longer changes as a tree switches between
//! leaf-by-leaf, near and middle levels. Middle-level trees casting their far level the same
//! way cut the shadow maps' triangles by 40% more at the forest trail, but blurred the dappled
//! light on the ground into blobs, so they cast themselves.
//!
//! In the coarse shadow maps, though, a texel is already wider than the gaps between the
//! middle level's cards ([`COARSE_TEXEL_M`] or more: the far cascades, the far shadow map and
//! the sky occlusion map), and the far level's coarser crown casts the same shadow there. So near, leaf-by-leaf and
//! middle parts cast at their middle level only into the fine maps
//! ([`ShadowTexelRange::finer_than`]) and a second stand-in casts their far level into the
//! coarse ones ([`ShadowTexelRange::from_texels`]): each map gets the tree exactly once, and
//! the switch sits at a cascade's edge, inside the cascades' blend, never at a distance from
//! the tree. Horizon Zero Dawn's one fixed vegetation shadow level per cascade (GDC 2018) and
//! Unreal's per-cascade caster LOD do the same. Far trees, the other way round, cast their
//! middle level into the finest maps ([`FAR_FINE_TEXEL_M`]) through a stand-in, so a tree's
//! shadow keeps its shape in the cascade where the view switches it between middle and far.
//! Research: engine-design/foliage-research.md, 4.1, 4.4, 15 and round 23.

use std::collections::HashMap;

use crate::assets::{FoliageVisuals, SpeciesVisual};
use crate::visuals::PlantPart;
use engine_assets::Handle;
use engine_core::prelude::*;
use engine_render::{
    MaterialRef, Mesh, MeshRef, MeshRenderer, ShadowOnly, ShadowTexelRange, StandardMaterial,
};

/// Shadow maps with texels this wide (meters) or wider get a tree's far level; finer ones its
/// middle level. About a screen pixel where the view itself switches to the far level (70 m
/// for a grown tree, at 1600 pixels across 90 degrees), so the coarse maps lose no more of the
/// crown than the view does there.
pub const COARSE_TEXEL_M: f32 = 0.1;

/// Shadow maps with texels finer than this (meters) get a far tree's middle level, through a
/// stand-in; coarser ones the far level itself. The cascade a tree switches level in (70 m for
/// a grown tree, 35 m for a small one): the second cascade at Ultra (4 cm texels) and at High
/// (5 cm). Without it a tree's shadow there changed shape as the view switched it (playtest 3
/// #3); with every fine map (under [`COARSE_TEXEL_M`]) the trail cast 28% more shadow
/// triangles at Ultra, for trees whose shadows shade ground past that cascade, where the far
/// crown casts the same shadow at that texel size. So the shadow changes at that cascade's
/// edge, inside the cascades' blend, not at the tree.
pub const FAR_FINE_TEXEL_M: f32 = 0.06;

/// Whether a plant part casts a shadow, its own or its stand-in's (without one: it does).
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct CastsShadow(pub bool);

/// A part's shadow stand-in: a child entity drawn into the shadow maps only.
#[derive(Component, Debug, Clone, Copy)]
pub struct StandIn(pub Entity);

/// A part's stand-in for the coarse shadow maps: a child casting its far level there.
#[derive(Component, Debug, Clone, Copy)]
pub struct CoarseStandIn(pub Entity);

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
    /// Near, leaf-by-leaf and middle meshes to their far-level ones, for the coarse maps, and
    /// the leaf materials to the far crown's mass-atlas ones.
    coarse_meshes: HashMap<u32, Handle<Mesh>>,
    coarse_materials: HashMap<u32, Handle<StandardMaterial>>,
    /// Far meshes to their middle-level ones, and the far crown's mass-atlas materials to the
    /// leaf ones, for the fine maps.
    finer_meshes: HashMap<u32, Handle<Mesh>>,
    finer_materials: HashMap<u32, Handle<StandardMaterial>>,
}

impl StandIns {
    /// Add a species' stand-ins.
    pub fn add(&mut self, sp: &crate::species::PlantSpecies, vis: &SpeciesVisual) {
        // A spruce's limbs hide under its needle shelves (its far level draws its trunk alone):
        // its shadows cast that trunk at every level, from 500 triangles instead of 6,000.
        let limbs_hidden = crate::meshgen::tree::limbs_hidden(sp);
        for v in &vis.variants {
            let mut map = |from: Option<Handle<Mesh>>, to: Option<Handle<Mesh>>| {
                if let (Some(from), Some(to)) = (from, to) {
                    self.meshes.insert(from.index(), to);
                }
            };
            if limbs_hidden && v.wood_lod.is_some() {
                map(v.wood, v.wood_lod);
                map(v.wood_mid, v.wood_lod);
            } else {
                map(v.wood, v.wood_mid.or(v.wood_lod));
            }
            map(v.foliage, v.foliage_mid);
            map(v.foliage_hero, v.foliage_mid);
            for (from, to) in v.foliage_thin.iter().zip(&v.foliage_mid_thin) {
                map(Some(*from), Some(*to));
            }
            // The far level as the view draws it: a trunk of its own, and a crown of its own
            // (species without one draw their near crown far off, so have no coarse crown).
            let mut coarse = |from: Option<Handle<Mesh>>, to: Option<Handle<Mesh>>| {
                if let (Some(from), Some(to)) = (from, to) {
                    self.coarse_meshes.insert(from.index(), to);
                }
            };
            for from in [v.wood, v.wood_mid] {
                coarse(from, v.wood_lod);
            }
            for from in [v.foliage, v.foliage_hero, v.foliage_mid] {
                coarse(from, v.foliage_lod);
            }
            for (i, to) in v.foliage_lod_thin.iter().enumerate() {
                coarse(v.foliage_thin.get(i).copied(), Some(*to));
                coarse(v.foliage_mid_thin.get(i).copied(), Some(*to));
            }
            // And back: a far tree casts its middle level into the fine maps.
            let mut finer = |from: Option<Handle<Mesh>>, to: Option<Handle<Mesh>>| {
                if let (Some(from), Some(to)) = (from, to)
                    && from != to
                {
                    self.finer_meshes.insert(from.index(), to);
                }
            };
            if !limbs_hidden {
                finer(v.wood_lod, v.wood_mid);
            }
            finer(v.foliage_lod, v.foliage_mid);
            for (from, to) in v.foliage_lod_thin.iter().zip(&v.foliage_mid_thin) {
                finer(Some(*from), Some(*to));
            }
        }
        if let Some(hero) = &vis.leaves_hero {
            for (from, to) in hero.iter().zip(&vis.leaves) {
                self.materials.insert(from.index(), *to);
            }
            for (from, to) in hero.iter().zip(&vis.leaves_far) {
                self.coarse_materials.insert(from.index(), *to);
            }
        }
        for (from, to) in vis.leaves.iter().zip(&vis.leaves_far) {
            self.coarse_materials.insert(from.index(), *to);
            self.finer_materials.insert(to.index(), *from);
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

    /// What a part drawing `mesh` with `material` casts into the coarse shadow maps with, if
    /// not with what it casts into the fine ones.
    pub fn coarse(
        &self,
        mesh: &MeshRef,
        material: &MaterialRef,
    ) -> Option<(Handle<Mesh>, Handle<StandardMaterial>)> {
        let (MeshRef::Handle(mesh), MaterialRef::Handle(material)) = (mesh, material) else {
            return None;
        };
        let to = *self.coarse_meshes.get(&mesh.index())?;
        let material = self
            .coarse_materials
            .get(&material.index())
            .copied()
            .unwrap_or(*material);
        Some((to, material))
    }

    /// What a part drawing a far-level `mesh` with `material` casts into the fine shadow maps
    /// with: its middle level.
    pub fn finer(
        &self,
        mesh: &MeshRef,
        material: &MaterialRef,
    ) -> Option<(Handle<Mesh>, Handle<StandardMaterial>)> {
        let (MeshRef::Handle(mesh), MaterialRef::Handle(material)) = (mesh, material) else {
            return None;
        };
        let to = *self.finer_meshes.get(&mesh.index())?;
        let material = self
            .finer_materials
            .get(&material.index())
            .copied()
            .unwrap_or(*material);
        Some((to, material))
    }
}

/// Give parts showing a near or leaf-by-leaf mesh a shadow stand-in, and drop it again when they
/// switch to a level that casts itself; and parts with a far level a second one casting it into
/// the coarse maps.
/// `FOLIAGE_TREE_SHADOWS=0` turns every plant's shadow off, for A/B runs of one binary that
/// weigh what plant shadows cost the GPU (on the cloud VM's lavapipe, 29% of the forest trail's
/// frame at Ultra, 2026-11-07). Read once.
fn plant_shadows_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("FOLIAGE_TREE_SHADOWS").map_or(true, |v| v != "0"))
}

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
            Option<&CoarseStandIn>,
            Option<&ShadowTexelRange>,
        ),
        (
            With<PlantPart>,
            Or<(Changed<MeshRenderer>, Changed<CastsShadow>)>,
        ),
    >,
    mut standins: Query<
        (&mut MeshRenderer, &mut ShadowTexelRange),
        (With<ShadowStandIn>, Without<PlantPart>),
    >,
) {
    for (entity, mut r, wants, standin, coarse_standin, range) in &mut parts {
        let wants = wants.is_none_or(|w| w.0) && r.visible && plant_shadows_on();
        let fine = wants
            .then(|| visuals.standins.get(&r.mesh, &r.material))
            .flatten();
        // The same caster for both (a spruce's trunk): one stand-in into every map.
        let coarse = wants
            .then(|| visuals.standins.coarse(&r.mesh, &r.material))
            .flatten()
            .filter(|c| Some(c) != fine.as_ref());
        // A far tree casts its middle level into the fine maps (through a stand-in) and itself
        // into the coarse ones: so its shadow doesn't change as the view switches it between
        // middle and far, only at a cascade's edge.
        let finer = (wants && fine.is_none() && coarse.is_none())
            .then(|| visuals.standins.finer(&r.mesh, &r.material))
            .flatten();
        // Whatever casts into the fine maps leaves the coarse ones to the far level.
        let fine_range = if finer.is_some() {
            ShadowTexelRange::finer_than(FAR_FINE_TEXEL_M)
        } else if coarse.is_some() {
            ShadowTexelRange::finer_than(COARSE_TEXEL_M)
        } else {
            ShadowTexelRange::default()
        };
        let own = wants && fine.is_none();
        if r.cast_shadows != own {
            r.cast_shadows = own;
        }
        let own_range = match (own, finer.is_some()) {
            (true, true) => ShadowTexelRange::from_texels(FAR_FINE_TEXEL_M),
            (true, false) => fine_range,
            (false, _) => ShadowTexelRange::default(),
        };
        let fine = fine.or(finer);
        if range.copied().unwrap_or_default() != own_range {
            if own_range == ShadowTexelRange::default() {
                commands.entity(entity).remove::<ShadowTexelRange>();
            } else {
                commands.entity(entity).insert(own_range);
            }
        }
        let fine_id = sync_standin(
            &mut commands,
            &mut standins,
            entity,
            standin.map(|s| s.0),
            fine.map(|f| (f, fine_range)),
        );
        match (fine_id, standin) {
            (Some(id), None) => {
                commands.entity(entity).insert(StandIn(id));
            }
            (None, Some(_)) => {
                commands.entity(entity).remove::<StandIn>();
            }
            _ => {}
        }
        let coarse_id = sync_standin(
            &mut commands,
            &mut standins,
            entity,
            coarse_standin.map(|s| s.0),
            coarse.map(|c| (c, ShadowTexelRange::from_texels(COARSE_TEXEL_M))),
        );
        match (coarse_id, coarse_standin) {
            (Some(id), None) => {
                commands.entity(entity).insert(CoarseStandIn(id));
            }
            (None, Some(_)) => {
                commands.entity(entity).remove::<CoarseStandIn>();
            }
            _ => {}
        }
    }
}

/// Make the stand-in `have` of part `parent` cast `want` (mesh, material, maps), spawning or
/// despawning it as needed. Returns the stand-in now there.
#[allow(clippy::type_complexity)]
fn sync_standin(
    commands: &mut Commands,
    standins: &mut Query<
        (&mut MeshRenderer, &mut ShadowTexelRange),
        (With<ShadowStandIn>, Without<PlantPart>),
    >,
    parent: Entity,
    have: Option<Entity>,
    want: Option<((Handle<Mesh>, Handle<StandardMaterial>), ShadowTexelRange)>,
) -> Option<Entity> {
    match (want, have) {
        (Some(((mesh, material), range)), Some(s)) => {
            if let Ok((mut r, mut maps)) = standins.get_mut(s) {
                let (mesh, material) = (MeshRef::Handle(mesh), MaterialRef::Handle(material));
                if r.mesh != mesh || r.material != material || !r.visible {
                    r.mesh = mesh;
                    r.material = material;
                    r.visible = true;
                }
                if *maps != range {
                    *maps = range;
                }
            }
            Some(s)
        }
        (Some(((mesh, material), range)), None) => {
            let mut r = MeshRenderer::new(
                engine_render::Shape::Cuboid { size: [0.01; 3] },
                StandardMaterial::default(),
            );
            r.mesh = MeshRef::Handle(mesh);
            r.material = MaterialRef::Handle(material);
            Some(
                commands
                    .spawn((
                        Transform::default(),
                        ChildOf(parent),
                        r,
                        ShadowOnly,
                        ShadowStandIn,
                        range,
                    ))
                    .id(),
            )
        }
        (None, Some(s)) => {
            // Gone rather than hidden: most trees spend most of their time far off, and a
            // hidden entity per tree part would still cost every frame's walk.
            if let Ok(mut e) = commands.get_entity(s) {
                e.despawn();
            }
            None
        }
        (None, None) => None,
    }
}
