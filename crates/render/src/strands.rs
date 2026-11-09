//! The one strand renderer for hair and fur: its API, reserved on day 0 of characters week
//! (engine-design/characters-week.md, Rendering's round R2), so Character (scalp hair, brows,
//! beards) and Softbody (animal fur) build against one set of types while the renderer is
//! written. Until R2 lands nothing draws a [`StrandInstance`]: clients keep their own draws and
//! switch over once it does.
//!
//! The shape follows the 2023-2025 shipped strand renderers (Frostbite's hair in Dragon Age:
//! The Veilguard, RE Engine, id Tech in Indiana Jones): a groom asset holds every strand's rest
//! shape and the guides a simulation moves; an instance places it on a body and picks a level
//! of detail. Up close each strand is drawn (shaded once per vertex, rasterized in compute with
//! real coverage); further out the groom is drawn as cards made from it; far away as shells
//! (fur) or nothing.
//!
//! Fields may be added before R2 lands: build these with `..Default::default()`.

use engine_assets::{Asset, Handle};
use engine_core::prelude::*;

/// What kind of fibre a groom is made of: it picks the scattering model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum StrandFiber {
    /// Human hair (Marschner R, TT, TRT lobes; scalp hair, brows, lashes, beards).
    #[default]
    Human,
    /// Animal fur: the human lobes plus the medulla's diffuse scattering inside the fibre (Yan
    /// et al. 2015, "Physically-accurate fur reflectance").
    Fur,
}

/// One strand's guides: up to three, nearest first, and how much of each one's motion it takes
/// (summing to 1). `u32::MAX`: no guide.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct StrandFollow {
    pub guides: [u32; 3],
    pub weights: [f32; 3],
}

impl Default for StrandFollow {
    fn default() -> Self {
        Self {
            guides: [u32::MAX; 3],
            weights: [0.0; 3],
        }
    }
}

/// A groom: every strand's rest shape in the model space of the body it grows on, and the
/// guides that move it. An asset (`Assets<StrandGroom>`), shared by every instance that wears
/// it.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StrandGroom {
    pub fiber: StrandFiber,
    /// Points per strand, root first; the same for every strand.
    pub points_per: u32,
    /// Per point: model-space position (m) and the fibre's radius there (m).
    pub points: Vec<[f32; 4]>,
    /// Per strand: its color at the root (linear RGB, the fibre's own pigment, before
    /// lighting) and its random value (0..1: grey hairs, color and width variation).
    pub roots: Vec<[f32; 4]>,
    /// Per strand: the color at its tip (linear RGB) and how rough its cuticle is (0..1).
    /// Empty: the root color all along, roughness from the fibre kind.
    pub tips: Vec<[f32; 4]>,
    /// Guides a simulation moves (`engine_world::HairGuides` order), `points_per` points each,
    /// model space. Empty for hair that holds its shape.
    pub guides: Vec<[f32; 3]>,
    /// Per strand, the guides it moves with (empty without guides).
    pub follow: Vec<StrandFollow>,
}

impl Asset for StrandGroom {}

impl StrandGroom {
    /// Strands in the groom.
    pub fn strand_count(&self) -> usize {
        self.points.len() / self.points_per.max(1) as usize
    }

    /// Guides in the groom.
    pub fn guide_count(&self) -> usize {
        self.guides.len() / self.points_per.max(1) as usize
    }

    /// Whether the arrays agree with `points_per` and each other (what the renderer checks
    /// before drawing a groom; one that doesn't is skipped with a warning).
    pub fn is_valid(&self) -> bool {
        let per = self.points_per as usize;
        let strands = self.strand_count();
        per >= 2
            && self.points.len() == strands * per
            && self.roots.len() == strands
            && (self.tips.is_empty() || self.tips.len() == strands)
            && self.guides.len().is_multiple_of(per)
            && (self.follow.is_empty() || self.follow.len() == strands)
            && self.follow.iter().all(|f| {
                f.guides
                    .iter()
                    .all(|&g| g == u32::MAX || (g as usize) < self.guide_count())
            })
    }
}

/// How a groom is drawn: a level of detail.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Serialize, Deserialize,
)]
pub enum StrandLod {
    /// Every strand (up close).
    #[default]
    Strands,
    /// Cards generated from the groom: textured ribbons along its clumps (a few meters out).
    Cards,
    /// Shells over the body's surface (fur, far away).
    Shells,
    /// Not drawn.
    Hidden,
}

/// A groom worn by this entity: drawn at the entity's `Transform`, its guides posed by the
/// entity's `engine_world::HairGuidesPosed` when that matches the groom (skinned with the body
/// otherwise).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StrandInstance {
    pub groom: Handle<StrandGroom>,
    /// A fixed level of detail; `None` picks one by distance and screen size.
    #[serde(default)]
    pub lod: Option<StrandLod>,
    /// Scales every strand's width (1 = as groomed).
    #[serde(default = "one")]
    pub width_scale: f32,
    /// Multiplies the groom's colors (dye, wetness darkening, dust).
    #[serde(default = "white")]
    pub tint: [f32; 3],
    #[serde(default = "yes")]
    pub visible: bool,
}

fn one() -> f32 {
    1.0
}

fn white() -> [f32; 3] {
    [1.0; 3]
}

fn yes() -> bool {
    true
}

impl StrandInstance {
    /// `groom` as groomed, its level of detail picked by distance.
    pub fn new(groom: Handle<StrandGroom>) -> Self {
        Self {
            groom,
            lod: None,
            width_scale: one(),
            tint: white(),
            visible: yes(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn groom(strands: usize, per: u32) -> StrandGroom {
        StrandGroom {
            points_per: per,
            points: vec![[0.0, 0.0, 0.0, 5e-5]; strands * per as usize],
            roots: vec![[0.1, 0.05, 0.02, 0.5]; strands],
            ..Default::default()
        }
    }

    #[test]
    fn counts_and_checks_follow_the_arrays() {
        let mut g = groom(10, 8);
        assert_eq!(g.strand_count(), 10);
        assert!(g.is_valid());
        g.guides = vec![[0.0; 3]; 16];
        g.follow = vec![
            StrandFollow {
                guides: [1, u32::MAX, u32::MAX],
                weights: [1.0, 0.0, 0.0],
            };
            10
        ];
        assert_eq!(g.guide_count(), 2);
        assert!(g.is_valid());
        g.follow[3].guides[0] = 2;
        assert!(!g.is_valid(), "a guide past the end");
        g.follow[3].guides[0] = 0;
        g.roots.pop();
        assert!(!g.is_valid(), "a strand without a root color");
        assert!(!groom(4, 1).is_valid(), "a strand needs two points");
    }

    #[test]
    fn grooms_round_trip_through_json() {
        let g = groom(2, 4);
        let back: StrandGroom = serde_json::from_str(&serde_json::to_string(&g).unwrap()).unwrap();
        assert_eq!(back, g);
        let i: StrandInstance = serde_json::from_str(r#"{"groom": 3}"#).unwrap();
        assert_eq!(i, StrandInstance::new(Handle::from_index(3)));
    }
}
