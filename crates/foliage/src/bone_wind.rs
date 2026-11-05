//! Trees and shrubs sway on their wind skeletons (F5 of engine-design/fields-and-forests-look.md;
//! foliage-wind-research.md §3): every variant's swaying meshes, baked on their skeleton in the
//! species build (`meshgen::tree::PlantMeshes::bake_wind_bones`), are handed to Rendering's
//! `PlantWindMeshes` with that skeleton, and every species' wind response ([`PlantWind`]) to its
//! slot in `PlantWindSpecies` (slot = the species' place in the library).

use std::sync::Arc;

use engine_assets::Handle;
use engine_core::prelude::*;
use engine_render::{
    Mesh, PlantBone, PlantSkeleton, PlantSpeciesWind, PlantWindMesh, PlantWindMeshes,
    PlantWindSpecies,
};

use crate::assets::{FoliageVisuals, VariantMeshes};
use crate::meshgen::wind_bones::WindSkeleton;
use crate::species::PlantLibrary;
use crate::wind::PlantWind;

/// Rendering's skeleton from ours (the drag shares stay on the CPU).
pub fn plant_skeleton(s: &WindSkeleton) -> PlantSkeleton {
    PlantSkeleton {
        bones: s
            .bones
            .iter()
            .map(|b| PlantBone {
                pivot: b.pivot,
                length: b.length,
                axis: b.axis,
                face: b.face,
                parent: b.parent,
                level: b.level,
                phase: b.phase,
                stiffness: b.stiffness,
            })
            .collect(),
    }
}

/// A species' slot in Rendering's plant wind.
pub fn species_wind(w: &PlantWind) -> PlantSpeciesWind {
    PlantSpeciesWind {
        f0_hz: w.f0_hz,
        damping: w.damping,
        stiffness: w.stiffness,
        limb_freq: w.limb_freq,
        branch_freq: w.branch_freq,
        flutter_hz: w.flutter_hz,
        flutter_ramp: w.flutter_ramp,
        branch_ramp: w.branch_ramp,
        crown_ramp: w.crown_ramp,
        flip: w.flip,
        underside_gain: w.underside_gain,
        ..PlantSpeciesWind::default()
    }
}

/// Every swaying mesh of a variant with its level of detail on Rendering's scale (3 near and
/// hero, 2 mid, 1 far, 0 distant) and whether it is the sapling's.
pub fn wind_meshes(v: &VariantMeshes) -> Vec<(&Handle<Mesh>, u8, bool)> {
    let single = [
        (&v.wood, 3, false),
        (&v.foliage, 3, false),
        (&v.foliage_hero, 3, false),
        (&v.foliage_dead, 3, false),
        (&v.wood_mid, 2, false),
        (&v.foliage_mid, 2, false),
        (&v.wood_lod, 1, false),
        (&v.foliage_lod, 1, false),
        (&v.foliage_dead_lod, 1, false),
        (&v.wood_distant, 0, false),
        (&v.foliage_distant, 0, false),
        (&v.sapling_wood, 3, true),
        (&v.sapling_foliage, 3, true),
    ];
    let thin = [
        (&v.foliage_thin, 3, false),
        (&v.foliage_mid_thin, 2, false),
        (&v.foliage_lod_thin, 1, false),
        (&v.foliage_distant_thin, 0, false),
        (&v.sapling_foliage_thin, 3, true),
    ];
    single
        .into_iter()
        .filter_map(|(h, detail, sapling)| h.as_ref().map(|h| (h, detail, sapling)))
        .chain(
            thin.into_iter()
                .flat_map(|(list, detail, sapling)| list.iter().map(move |h| (h, detail, sapling))),
        )
        .collect()
}

/// What [`register_bone_wind`] registered for one species: built from what, and its meshes.
#[derive(Default)]
pub struct Registered {
    built_from: Option<crate::species::PlantSpecies>,
    meshes: Vec<Handle<Mesh>>,
}

/// Hand each newly built species' meshes and wind response to Rendering, taking back the
/// meshes of the build it replaces (their slots in the mesh store get reused).
pub fn register_bone_wind(
    library: Res<PlantLibrary>,
    visuals: Res<FoliageVisuals>,
    mut meshes: ResMut<PlantWindMeshes>,
    mut species: ResMut<PlantWindSpecies>,
    mut done: Local<Vec<Registered>>,
) {
    if !visuals.is_changed() && !library.is_changed() {
        return;
    }
    let len = visuals.species.len().max(done.len());
    done.resize_with(len, Registered::default);
    if species.slots.len() < library.len() {
        species
            .slots
            .resize(library.len(), PlantSpeciesWind::default());
    }
    for (i, r) in done.iter_mut().enumerate() {
        let v = visuals.species.get(i).and_then(|v| v.as_ref());
        if v.map(|v| &v.built_from) == r.built_from.as_ref() {
            continue;
        }
        for h in r.meshes.drain(..) {
            meshes.remove(h);
        }
        r.built_from = v.map(|v| v.built_from.clone());
        let Some(v) = v else { continue };
        if let Some(slot) = species.slots.get_mut(i) {
            *slot = species_wind(&v.built_from.wind);
        }
        for variant in &v.variants {
            let tree = variant.wind.as_deref().map(|s| Arc::new(plant_skeleton(s)));
            let sapling = variant
                .sapling_wind
                .as_deref()
                .map(|s| Arc::new(plant_skeleton(s)));
            for (h, detail, is_sapling) in wind_meshes(variant) {
                let Some(skeleton) = (if is_sapling { &sapling } else { &tree }) else {
                    continue;
                };
                meshes.insert(
                    *h,
                    PlantWindMesh {
                        skeleton: skeleton.clone(),
                        species: i as u16,
                        detail,
                    },
                );
                r.meshes.push(*h);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skeletons_and_wind_carry_over() {
        let lib = PlantLibrary::default();
        let oak = lib
            .species
            .iter()
            .find(|s| s.name.contains("oak"))
            .expect("an oak");
        let m = crate::meshgen::tree::woody(oak, 7, false);
        let s = m.wind.as_deref().expect("a skeleton");
        let p = plant_skeleton(s);
        assert_eq!(p.bones.len(), s.bones.len());
        for (a, b) in p.bones.iter().zip(&s.bones) {
            assert_eq!((a.pivot, a.parent, a.level), (b.pivot, b.parent, b.level));
        }
        let w = species_wind(&oak.wind);
        assert_eq!(w.f0_hz, oak.wind.f0_hz);
        assert_eq!(w.flip, oak.wind.flip);
        // The renderer's word is ours.
        assert_eq!(
            engine_render::pack_plant_vertex(77, 0.4, 0.6, true),
            crate::meshgen::wind_bones::pack(77, 0.4, 0.6, true)
        );
    }
}
