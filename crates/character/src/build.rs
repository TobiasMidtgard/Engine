//! A whole character from an [`Appearance`]: skin, eyes and hair, in the character's model
//! space (Y up, facing +Z, the base mesh's own origin near the hips).

use engine_core::math::Vec3;

use crate::appearance::Appearance;
use crate::eyes::{self, EyeMesh};
use crate::mesh::{self, Landmarks, Region, Regions, SkinMesh};
use crate::pack::HumanPack;
use crate::strands::{self, StrandSet};

/// Everything drawn for one character.
#[derive(Debug, Clone, Default)]
pub struct CharacterMeshes {
    pub skin: SkinMesh,
    pub eyes: EyeMesh,
    pub strands: Vec<StrandSet>,
    /// Landmarks moved with the morphs (model space).
    pub landmarks: Option<Landmarks>,
}

impl CharacterMeshes {
    /// Triangles drawn (skin, eyes, and two per hair segment).
    pub fn triangles(&self) -> usize {
        let hair: usize = self
            .strands
            .iter()
            .map(|s| s.strand_count() * (s.points_per.saturating_sub(1) as usize) * 2)
            .sum();
        self.skin.indices.len() / 3 + self.eyes.indices.len() / 3 + hair
    }
}

/// Options for [`build`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BuildOptions {
    pub region: Region,
    /// Subdivision levels of the skin (2 for close-ups, 1 for people across a room).
    pub levels: u32,
    /// Grow hair strands (scalp, brows, lashes, beard).
    pub hair: bool,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            region: Region::Bust,
            levels: 2,
            hair: true,
        }
    }
}

/// Build a character.
pub fn build(
    pack: &HumanPack,
    regions: &Regions,
    a: &Appearance,
    opts: BuildOptions,
) -> CharacterMeshes {
    let (skin, morphed) = mesh::build_skin(pack, regions, a, opts.region, opts.levels);
    let lm0 = regions.landmarks;
    let eye_center = |side: &str, fallback: Vec3| {
        let v = pack.group_vertices(&format!("helper-{side}-eye"));
        if v.is_empty() {
            return fallback;
        }
        v.iter().map(|i| morphed[*i as usize]).sum::<Vec3>() / v.len() as f32
    };
    let le = eye_center("l", lm0.eyes[0]);
    let re = eye_center("r", lm0.eyes[1]);
    // The eye radius scales with the head (helpers move with the morphs).
    let helper = pack.group_vertices("helper-l-eye");
    let radius = if helper.is_empty() {
        lm0.eye_radius
    } else {
        helper
            .iter()
            .map(|i| (morphed[*i as usize] - le).length())
            .fold(0.0f32, f32::max)
    };
    let eyes_mesh = eyes::build([le, re], radius * eyes::RADIUS_SCALE);
    let shift = |p: Vec3| p + ((le + re) * 0.5 - (lm0.eyes[0] + lm0.eyes[1]) * 0.5);
    let landmarks = Landmarks {
        eyes: [le, re],
        eye_radius: radius * eyes::RADIUS_SCALE,
        head: shift(lm0.head),
        mouth: shift(lm0.mouth),
    };
    let strands = if opts.hair {
        let margins = regions.lid_margins.clone().map(|lids| {
            lids.map(|lid| lid.iter().map(|v| morphed[*v as usize]).collect::<Vec<_>>())
        });
        strands::grow_all(a, &skin, &margins, &landmarks, &lm0)
    } else {
        Vec::new()
    };
    CharacterMeshes {
        skin,
        eyes: eyes_mesh,
        strands,
        landmarks: Some(landmarks),
    }
}
