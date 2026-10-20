//! A whole character from an [`Appearance`]: skin, eyes and hair, in the character's model
//! space (Y up, facing +Z, the base mesh's own origin near the hips).

use engine_core::math::Vec3;

use crate::appearance::Appearance;
use crate::eyes::{self, EyeMesh};
use crate::face::{self, FaceShapes};
use crate::mesh::{self, Landmarks, Region, Regions, SkinMesh};
use crate::mouth;
use crate::pack::HumanPack;
use crate::rig::Rig;
use crate::strands::{self, StrandSet};

/// Everything drawn for one character.
#[derive(Debug, Clone, Default)]
pub struct CharacterMeshes {
    pub skin: SkinMesh,
    pub eyes: EyeMesh,
    pub strands: Vec<StrandSet>,
    /// Per strand set, per hair: the skin vertex its root grows from (it follows that
    /// vertex's expressions and bones).
    pub strand_roots: Vec<Vec<u32>>,
    /// Landmarks moved with the morphs (model space).
    pub landmarks: Option<Landmarks>,
    /// The skeleton the skin is weighted to (`None` when the pack has no rig).
    pub rig: Option<Rig>,
    /// Expression shapes (empty when built without a face).
    pub face: FaceShapes,
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
    /// Build the expression shapes (a face that can move).
    pub face: bool,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            region: Region::Bust,
            levels: 2,
            hair: true,
            face: true,
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
    let (mut skin, morphed) = mesh::build_skin(pack, regions, a, opts.region, opts.levels);
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
    let strand_roots = strands
        .iter()
        .map(|set| roots(&skin.positions, set))
        .collect();
    if opts.face {
        skin.add_mouth(pack, &mouth::build(pack, &morphed));
    }
    let face = if opts.face {
        face::build(pack, &skin, a)
    } else {
        FaceShapes::default()
    };
    CharacterMeshes {
        rig: Rig::new(pack, &morphed),
        skin,
        eyes: eyes_mesh,
        strands,
        strand_roots,
        landmarks: Some(landmarks),
        face,
    }
}

/// The skin vertex nearest each hair's root.
fn roots(skin: &[Vec3], set: &StrandSet) -> Vec<u32> {
    const CELL: f32 = 0.004;
    let key = |p: Vec3| {
        let c = (p / CELL).floor();
        (c.x as i32, c.y as i32, c.z as i32)
    };
    let mut grid: std::collections::HashMap<(i32, i32, i32), Vec<u32>> =
        std::collections::HashMap::new();
    for (i, p) in skin.iter().enumerate() {
        grid.entry(key(*p)).or_default().push(i as u32);
    }
    let per = set.points_per.max(1) as usize;
    (0..set.strand_count())
        .map(|h| {
            let r = set.points[h * per];
            let root = Vec3::new(r[0], r[1], r[2]);
            let k = key(root);
            let mut best = (f32::MAX, 0u32);
            for reach in 1..=4 {
                for dx in -reach..=reach {
                    for dy in -reach..=reach {
                        for dz in -reach..=reach {
                            let Some(cell) = grid.get(&(k.0 + dx, k.1 + dy, k.2 + dz)) else {
                                continue;
                            };
                            for &i in cell {
                                let d = (skin[i as usize] - root).length_squared();
                                if d < best.0 {
                                    best = (d, i);
                                }
                            }
                        }
                    }
                }
                if best.0 < f32::MAX {
                    break;
                }
            }
            best.1
        })
        .collect()
}
