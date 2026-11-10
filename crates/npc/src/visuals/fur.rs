//! Wildlife coats for the one fur renderer (characters week S1): each coated piece of a deer,
//! wolf, boar or rabbit wears an `engine_world::FurCoat` built from its mesh (rigid, no bones),
//! which the hair module grows strands on and draws, all of an animal's pieces as one. Coats
//! are built once per mesh and paint, so a herd shares one set of arrays.

use std::collections::HashMap;

use engine_assets::{Assets, Handle};
use engine_core::prelude::*;
use engine_render::{MaterialRef, Mesh, MeshRef, MeshRenderer};
use engine_world::{FurCoat, FurCoatDrawer, FurStyle};

use super::animal::{CROUP, GLOSS, LEG_FOOT, LEG_LOW, MANE, RUMP, TAIL_FAN, TAIL2, TUSKS};
use super::{Part, StandInPart};
use crate::wildlife::{Animal, Species};

/// A part that grows no fur (eyes and nose, antlers, tusks, feathers).
#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct NoCoat;

/// The species' coat, or `None` (birds wear feathers).
fn style(species: Species) -> Option<FurStyle> {
    let base = FurStyle {
        radius: 0.00005,
        stiffness: 0.9,
        curl: 0.0,
        ..Default::default()
    };
    Some(match species {
        Species::Deer => FurStyle {
            length: 0.022,
            density: 250.0,
            tip: [1.15, 1.1, 1.02],
            ..base
        },
        Species::Wolf => FurStyle {
            length: 0.045,
            density: 250.0,
            // Agouti: dark-tipped guard hairs.
            tip: [0.72, 0.7, 0.68],
            stiffness: 0.8,
            ..base
        },
        Species::Boar => FurStyle {
            length: 0.05,
            density: 80.0,
            // Grizzled: pale-banded bristles.
            tip: [1.35, 1.3, 1.24],
            radius: 0.00009,
            stiffness: 0.85,
            curl: 0.002,
        },
        Species::Rabbit => FurStyle {
            length: 0.018,
            density: 400.0,
            tip: [1.12, 1.08, 1.0],
            stiffness: 0.85,
            ..base
        },
        Species::Bird => return None,
    })
}

/// A part's hair length as a multiple of its species' (`None`: no fur).
fn length_of(species: Species, part: Part) -> Option<f32> {
    Some(match part {
        Part::Head | Part::Snout | Part::Ear(_) => 0.6,
        Part::Tail => match species {
            Species::Wolf => 2.0,
            Species::Boar => 1.6,
            _ => 1.4,
        },
        Part::Leg(_) => 0.9,
        Part::Beast(b) if b == MANE || b == MANE + 1 => 2.4,
        Part::Beast(b) if b == TAIL2 => 2.0,
        Part::Beast(b) if (LEG_LOW..LEG_FOOT + 4).contains(&b) => 0.6,
        Part::Beast(b)
            if b == GLOSS || b == TUSKS || b == TUSKS + 3 || (TAIL_FAN..CROUP).contains(&b) =>
        {
            return None;
        }
        Part::Beast(b) if b == RUMP => 1.0,
        Part::Antler(_) | Part::Wing(_) => return None,
        _ => 1.0,
    })
}

/// The coat of a piece's mesh: colour from its paint (vertex colours times the material's),
/// lying along the texture's +V (the fur's flow), none on the hoof swatch.
pub(crate) fn coat_of(mesh: &Mesh, base: Vec3, style: FurStyle, length: f32) -> Option<FurCoat> {
    if mesh.vertices.is_empty() || mesh.indices.len() < 3 {
        return None;
    }
    let n = mesh.vertices.len();
    let pos = |i: usize| Vec3::from(mesh.vertices[i].position);
    let uv = |i: usize| Vec2::from(mesh.vertices[i].uv);
    // dP/dV per vertex (the way the painted fur lies).
    let mut comb = vec![Vec3::ZERO; n];
    for t in mesh.indices.as_chunks::<3>().0 {
        let [a, b, c] = t.map(|i| i as usize);
        if a.max(b).max(c) >= n {
            continue;
        }
        let (e1, e2) = (pos(b) - pos(a), pos(c) - pos(a));
        let (d1, d2) = (uv(b) - uv(a), uv(c) - uv(a));
        let det = d1.x * d2.y - d2.x * d1.y;
        if det.abs() < 1e-12 {
            continue;
        }
        let dv = (e2 * d1.x - e1 * d2.x) / det;
        for v in [a, b, c] {
            comb[v] += dv;
        }
    }
    let srgb_avg = 0.82; // the fur texture's mean brightness over its tufts and gaps
    Some(FurCoat {
        version: 1,
        positions: (0..n).map(pos).collect::<Vec<_>>().into(),
        normals: mesh
            .vertices
            .iter()
            .map(|v| Vec3::from(v.normal))
            .collect::<Vec<_>>()
            .into(),
        indices: mesh.indices.clone().into(),
        joints: Vec::new().into(),
        weights: Vec::new().into(),
        grown_on: Vec::new().into(),
        color: mesh
            .vertices
            .iter()
            .map(|v| (Vec3::from_slice(&v.color[..3]) * base * srgb_avg).to_array())
            .collect::<Vec<_>>()
            .into(),
        amount: mesh
            .vertices
            .iter()
            .map(|v| {
                if crate::art::animals::in_hoof_swatch(Vec2::from(v.uv), 512) {
                    0.0
                } else {
                    1.0
                }
            })
            .collect::<Vec<_>>()
            .into(),
        length: vec![length; n].into(),
        comb: comb
            .into_iter()
            .map(|c| c.normalize_or_zero())
            .collect::<Vec<_>>()
            .into(),
        style,
    })
}

/// Give each coated piece of a wild animal its coat (built once per mesh and paint).
#[allow(clippy::type_complexity)]
pub(crate) fn export_wild_coats(
    mut commands: Commands,
    parts: Query<
        (Entity, &StandInPart, &MeshRenderer, &ChildOf),
        (Without<FurCoat>, Without<NoCoat>),
    >,
    animals: Query<&Animal>,
    meshes: Option<Res<Assets<Mesh>>>,
    drawer: Option<Res<FurCoatDrawer>>,
    mut made: Local<HashMap<(Handle<Mesh>, Part, [u32; 3]), Option<FurCoat>>>,
) {
    // Nothing draws coats without a fur drawer (the parts keep their painted fur).
    let (Some(meshes), Some(_)) = (meshes, drawer) else {
        return;
    };
    for (e, part, renderer, parent) in &parts {
        let Ok(animal) = animals.get(parent.parent()) else {
            continue;
        };
        let (Some(style), Some(length), MeshRef::Handle(handle)) = (
            style(animal.species),
            length_of(animal.species, part.part),
            &renderer.mesh,
        ) else {
            commands.entity(e).insert(NoCoat);
            continue;
        };
        let base = match &part.base {
            MaterialRef::Inline(m) => Vec3::from_slice(&m.base_color[..3]),
            _ => Vec3::ONE,
        };
        let key = (*handle, part.part, base.to_array().map(f32::to_bits));
        let coat = match made.get(&key) {
            Some(c) => c.clone(),
            None => {
                let Some(mesh) = meshes.get(*handle) else {
                    continue;
                };
                let c = coat_of(mesh, base, style, length);
                made.insert(key, c.clone());
                c
            }
        };
        match coat {
            Some(c) => commands.entity(e).insert(c),
            None => commands.entity(e).insert(NoCoat),
        };
    }
}
