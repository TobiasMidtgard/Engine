//! The character contract: what a person looks like, kept apart from how they move, so the
//! look (the character module) and the motion (the animation module) can each be replaced.
//!
//! * [`Appearance`]: a person's whole look as plain data (body shape, face sliders, skin, eyes,
//!   hair, wear, headgear). A character creator, a save game, NPC spawning or an AI agent writes
//!   it; the character module builds and draws the body from it.
//! * [`AppearanceRequest`]: for spawners that don't pick looks themselves (a squad, a village):
//!   a stored seed plus a few constraints. The character module answers by inserting an
//!   [`Appearance`] it chose; members of one `family` look related but distinct.
//! * [`FaceControls`]: expression weights by ARKit blendshape name plus gaze. The animation
//!   module (or NPC AI) writes them every frame; the character module applies them.
//! * [`CharacterQuality`]: an optional override of how detailed a person is drawn; without it
//!   the character module picks the level from the camera distance.
//! * [`AppearanceBodies`]: present while a module draws every [`Appearance`] (see
//!   [`draws_appearances`]). While it exists, the animation module keeps writing
//!   [`SkeletonPose`](crate::animation::SkeletonPose) and [`FaceControls`] for entities with an
//!   [`Appearance`] but doesn't draw a mesh of its own for them. The drawing module skins its
//!   body to that pose by the animation module's humanoid bone names (listed in engine_anim's
//!   README) and owns the mapping from its own rig to them. Without a character module the
//!   animation module's bodies stay as they are.
//! * [`HairGuides`]: the guide strands the character module grew on a person, for a hair
//!   simulation (the softbody module) to move; it writes them back posed as
//!   [`HairGuidesPosed`]. Without a simulation the character module draws its hair from the
//!   rest guides skinned to the pose.
//!
//! Catalogs that will grow (hair and beard styles, headgear) are string ids; unknown ids fall
//! back to the default. Seeds are stored at spawn, never taken from entity ids.

use std::collections::BTreeMap;

use engine_core::prelude::*;

use crate::animation::BonePose;

/// A whole person's look. Every field has a default, so data only needs what differs.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    /// Varies what has no field (pore and freckle placement, which hairs grow where), the same
    /// way every run.
    pub seed: u64,
    /// 0 female .. 1 male, blending body and face shapes.
    pub sex: f32,
    /// Age, years (1..90). Shapes the skull, face and body, and ages the skin.
    pub age: f32,
    /// 0 slight .. 0.5 average .. 1 muscular.
    pub muscle: f32,
    /// 0 thin .. 0.5 average .. 1 heavy.
    pub weight: f32,
    /// 0 short .. 0.5 average .. 1 tall, relative to the average for the sex and age.
    pub height: f32,
    /// 0 uncommon .. 0.5 regular .. 1 idealized body proportions.
    pub proportions: f32,
    /// Face and body shape by ancestry.
    pub ancestry: Ancestry,
    /// Named face, neck and torso sliders (`nose-hump-decr|incr`, `head-oval`), -1..1, or 0..1
    /// for one-sided ones. Missing names are 0; the character module lists its names.
    pub shape: BTreeMap<String, f32>,
    pub skin: Skin,
    pub eyes: Eyes,
    pub hair: Hair,
    pub brows: Brows,
    pub beard: Beard,
    /// Dirt, blood, bruises and wetness on the skin and hair.
    pub wear: Wear,
    /// What covers the head. Built-in ids: `none`, `hood`, `open_helm` (hides hair, ears and
    /// neck, keeps the face from brow to chin), `closed_helm` (the head is not drawn).
    pub headgear: String,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            seed: 0,
            sex: 0.5,
            age: 25.0,
            muscle: 0.5,
            weight: 0.5,
            height: 0.5,
            proportions: 0.5,
            ancestry: Ancestry::default(),
            shape: BTreeMap::new(),
            skin: Skin::default(),
            eyes: Eyes::default(),
            hair: Hair::default(),
            brows: Brows::default(),
            beard: Beard::default(),
            wear: Wear::default(),
            headgear: "none".into(),
        }
    }
}

/// Relative weights of three ancestry shape sets (normalized when used; all 0 = equal).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ancestry {
    /// 0..1, relative.
    pub african: f32,
    /// 0..1, relative.
    pub asian: f32,
    /// 0..1, relative.
    pub european: f32,
}

impl Default for Ancestry {
    fn default() -> Self {
        Self {
            african: 1.0 / 3.0,
            asian: 1.0 / 3.0,
            european: 1.0 / 3.0,
        }
    }
}

/// Skin, from the two pigments that color real skin.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Skin {
    /// Melanin, 0 very fair .. 1 very dark.
    pub melanin: f32,
    /// Blood under the skin, 0 pale .. 1 ruddy.
    pub redness: f32,
    /// -1 cool (pink) .. 1 warm (olive, golden).
    pub undertone: f32,
    /// Freckles, 0 none .. 1 many.
    pub freckles: f32,
    /// Moles and small marks, 0 none .. 1 many.
    pub moles: f32,
    /// Shine, 0 matte .. 1 oily.
    pub oiliness: f32,
    /// Wrinkles beyond what the age gives, -1 smooth .. 1 deeply lined.
    pub wrinkles: f32,
    /// Pore size, 0 fine .. 1 coarse.
    pub pores: f32,
}

impl Default for Skin {
    fn default() -> Self {
        Self {
            melanin: 0.25,
            redness: 0.4,
            undertone: 0.1,
            freckles: 0.0,
            moles: 0.2,
            oiliness: 0.35,
            wrinkles: 0.0,
            pores: 0.45,
        }
    }
}

/// Eyes.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Eyes {
    /// Iris melanin, 0 blue .. 0.35 green or hazel .. 1 dark brown.
    pub melanin: f32,
    /// Extra yellow-green in the iris, 0..1.
    pub lipochrome: f32,
    /// Darkness of the ring around the iris, 0..1.
    pub limbal_ring: f32,
    /// Pupil opening, 0 small .. 1 wide.
    pub pupil: f32,
    /// Redness of the whites, 0..1.
    pub redness: f32,
}

impl Default for Eyes {
    fn default() -> Self {
        Self {
            melanin: 0.6,
            lipochrome: 0.2,
            limbal_ring: 0.6,
            pupil: 0.4,
            redness: 0.15,
        }
    }
}

/// Hair color from its pigments, plus grey.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HairColor {
    /// Eumelanin, 0 platinum .. 0.3 blonde .. 0.6 brown .. 1 black.
    pub melanin: f32,
    /// Share of red pheomelanin, 0 ash .. 1 auburn or ginger.
    pub red: f32,
    /// Grey or white hairs mixed in, 0..1.
    pub grey: f32,
}

impl Default for HairColor {
    fn default() -> Self {
        Self {
            melanin: 0.65,
            red: 0.2,
            grey: 0.0,
        }
    }
}

/// Scalp hair.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hair {
    /// Style id. Built-in: `bald`, `buzz`, `short` (default), `medium`, `long`, `bun`.
    pub style: String,
    pub color: HairColor,
    /// Length multiplier for the style, 0.5..1.5.
    pub length: f32,
    /// 0 straight .. 1 curly.
    pub curl: f32,
    /// Hairline, 0 full .. 1 receded.
    pub recession: f32,
}

impl Default for Hair {
    fn default() -> Self {
        Self {
            style: "short".into(),
            color: HairColor::default(),
            length: 1.0,
            curl: 0.15,
            recession: 0.0,
        }
    }
}

/// Eyebrows.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Brows {
    /// 0 sparse .. 1 thick.
    pub thickness: f32,
    /// `None` takes the scalp hair's color.
    pub color: Option<HairColor>,
}

impl Default for Brows {
    fn default() -> Self {
        Self {
            thickness: 0.5,
            color: None,
        }
    }
}

/// Facial hair.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Beard {
    /// Style id. Built-in: `none` (default), `stubble`, `short`, `full`, `goatee`.
    pub style: String,
    /// 0 patchy .. 1 dense.
    pub density: f32,
    /// `None` takes the scalp hair's color.
    pub color: Option<HairColor>,
}

impl Default for Beard {
    fn default() -> Self {
        Self {
            style: "none".into(),
            density: 0.8,
            color: None,
        }
    }
}

/// What a day has done to someone, 0 clean .. 1 heavy for each. NPC AI writes blood and
/// bruises, weather and water write wet, anyone may write dirt.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Wear {
    /// Dust, mud and grime, 0..1.
    pub dirt: f32,
    /// Blood on the skin, 0..1.
    pub blood: f32,
    /// Bruises and swelling, 0..1.
    pub bruises: f32,
    /// Wet skin and hair, 0..1.
    pub wet: f32,
}

/// Ask for a person without choosing their look. The character module inserts an
/// [`Appearance`] chosen from these; an entity that already has one keeps it.
#[derive(Component, Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AppearanceRequest {
    /// The person's own seed, stored at spawn (never an entity id).
    pub seed: u64,
    /// People sharing a family seed (a squad, a household) look related but distinct.
    pub family: Option<u64>,
    /// 0 female .. 1 male; `None` picks.
    pub sex: Option<f32>,
    /// Age range, years; `None` picks an adult.
    pub age: Option<[f32; 2]>,
    /// Ancestry weights; `None` picks.
    pub ancestry: Option<Ancestry>,
}

/// The ARKit blendshape names, in ARKit order. `Left` and `Right` are the character's own.
pub const ARKIT: [&str; 52] = [
    "eyeBlinkLeft",
    "eyeLookDownLeft",
    "eyeLookInLeft",
    "eyeLookOutLeft",
    "eyeLookUpLeft",
    "eyeSquintLeft",
    "eyeWideLeft",
    "eyeBlinkRight",
    "eyeLookDownRight",
    "eyeLookInRight",
    "eyeLookOutRight",
    "eyeLookUpRight",
    "eyeSquintRight",
    "eyeWideRight",
    "jawForward",
    "jawLeft",
    "jawRight",
    "jawOpen",
    "mouthClose",
    "mouthFunnel",
    "mouthPucker",
    "mouthLeft",
    "mouthRight",
    "mouthSmileLeft",
    "mouthSmileRight",
    "mouthFrownLeft",
    "mouthFrownRight",
    "mouthDimpleLeft",
    "mouthDimpleRight",
    "mouthStretchLeft",
    "mouthStretchRight",
    "mouthRollLower",
    "mouthRollUpper",
    "mouthShrugLower",
    "mouthShrugUpper",
    "mouthPressLeft",
    "mouthPressRight",
    "mouthLowerDownLeft",
    "mouthLowerDownRight",
    "mouthUpperUpLeft",
    "mouthUpperUpRight",
    "browDownLeft",
    "browDownRight",
    "browInnerUp",
    "browOuterUpLeft",
    "browOuterUpRight",
    "cheekPuff",
    "cheekSquintLeft",
    "cheekSquintRight",
    "noseSneerLeft",
    "noseSneerRight",
    "tongueOut",
];

/// Indices into [`ARKIT`] and [`FaceControls::weights`] for the shapes most code drives.
pub mod arkit {
    pub const EYE_BLINK_LEFT: usize = 0;
    pub const EYE_SQUINT_LEFT: usize = 5;
    pub const EYE_WIDE_LEFT: usize = 6;
    pub const EYE_BLINK_RIGHT: usize = 7;
    pub const EYE_SQUINT_RIGHT: usize = 12;
    pub const EYE_WIDE_RIGHT: usize = 13;
    pub const JAW_OPEN: usize = 17;
    pub const MOUTH_CLOSE: usize = 18;
    pub const MOUTH_SMILE_LEFT: usize = 23;
    pub const MOUTH_SMILE_RIGHT: usize = 24;
    pub const MOUTH_FROWN_LEFT: usize = 25;
    pub const MOUTH_FROWN_RIGHT: usize = 26;
    pub const MOUTH_STRETCH_LEFT: usize = 29;
    pub const MOUTH_STRETCH_RIGHT: usize = 30;
    pub const MOUTH_PRESS_LEFT: usize = 35;
    pub const MOUTH_PRESS_RIGHT: usize = 36;
    pub const BROW_DOWN_LEFT: usize = 41;
    pub const BROW_DOWN_RIGHT: usize = 42;
    pub const BROW_INNER_UP: usize = 43;
    pub const CHEEK_SQUINT_LEFT: usize = 47;
    pub const CHEEK_SQUINT_RIGHT: usize = 48;
    pub const NOSE_SNEER_LEFT: usize = 49;
    pub const NOSE_SNEER_RIGHT: usize = 50;
}

/// A face's expression this frame. The animation module (or NPC AI through it) writes it; the
/// character module applies it to the face it draws.
#[derive(Component, Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FaceControls {
    /// Blendshape weights in [`ARKIT`] order, 0..1. Shorter lists mean the rest are 0.
    pub weights: Vec<f32>,
    /// Where the eyes look relative to the head, radians: positive = toward the character's
    /// left (like yaw).
    pub gaze_yaw: f32,
    /// Where the eyes look relative to the head, radians: positive = up.
    pub gaze_pitch: f32,
}

impl FaceControls {
    /// The weight of a shape by ARKit name (0 for unknown names or unset shapes).
    pub fn get(&self, name: &str) -> f32 {
        ARKIT
            .iter()
            .position(|n| *n == name)
            .and_then(|i| self.weights.get(i).copied())
            .unwrap_or(0.0)
    }

    /// Sets a shape by ARKit name; returns false for an unknown name.
    pub fn set(&mut self, name: &str, weight: f32) -> bool {
        let Some(i) = ARKIT.iter().position(|n| *n == name) else {
            return false;
        };
        self.set_index(i, weight);
        true
    }

    /// Sets a shape by index (see [`arkit`]).
    pub fn set_index(&mut self, i: usize, weight: f32) {
        if i >= ARKIT.len() {
            return;
        }
        if self.weights.len() <= i {
            self.weights.resize(ARKIT.len(), 0.0);
        }
        self.weights[i] = weight;
    }
}

/// How detailed a person is drawn, when something wants to decide instead of the camera
/// distance (the creator's close-up, a crowd).
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CharacterQuality {
    /// Full detail: subdivided skin, every hair. Within a few meters.
    #[default]
    CloseUp,
    /// Coarser mesh and fewer, thicker hairs, for a few people at 5..30 m.
    Mid,
    /// Cheapest: for 20+ people at 30..60 m and beyond.
    Crowd,
}

/// Present while a module draws every [`Appearance`] (see [`draws_appearances`]); the animation
/// module then poses those people but doesn't draw its own mesh for them. Check for it when
/// systems run, not in `Plugin::build`, since the drawing plugin may be added later.
#[derive(Resource, Debug, Clone, Serialize, Deserialize)]
pub struct AppearanceBodies {
    pub drawn_by: String,
}

/// Call from a character plugin's `build` when it draws every [`Appearance`].
pub fn draws_appearances(app: &mut App, name: &str) {
    if app.world.get_resource::<AppearanceBodies>().is_none() {
        app.insert_resource(AppearanceBodies {
            drawn_by: name.to_string(),
        });
    }
}

/// Guide strands a look module grew on a person, for a hair simulation to move (each lock of
/// drawn hairs follows its guide). Replace the component, or bump `version`, when the hair is
/// regrown.
#[derive(Component, Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HairGuides {
    pub version: u32,
    pub guides: Vec<HairGuide>,
    /// The person's `SkeletonPose` bones (model space) when the hair was grown, one per bone of
    /// that pose: each guide rides its bones relative to this.
    pub grown_on: Vec<BonePose>,
    /// 0..1: how firmly the hair holds its grown shape (long hair ~0.2, short ~0.6, fur ~0.9).
    pub stiffness: f32,
}

/// One guide strand.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HairGuide {
    /// Indices into the person's `SkeletonPose` bones; `blend` is the weight of `bones[1]`
    /// (0 = only `bones[0]`).
    pub bones: [u16; 2],
    pub blend: f32,
    /// Root first, in the person's model space at `HairGuides::grown_on`.
    pub points: Vec<Vec3>,
    /// The skin's outward normal at the root, model space.
    pub normal: Vec3,
    /// The lock's thickness at the root, meters (for collisions).
    pub thickness: f32,
}

impl HairGuides {
    /// Points over all guides: the length of a matching [`HairGuidesPosed::points`].
    pub fn point_count(&self) -> usize {
        self.guides.iter().map(|g| g.points.len()).sum()
    }
}

/// Written by the hair simulation every fixed step: every guide's points in the person's model
/// space (the space of [`HairGuides`] and `SkeletonPose`, through the body's transform at that
/// step), all guides concatenated in [`HairGuides`] order. `version` echoes the
/// [`HairGuides::version`] simulated; the character module draws from these only while it
/// matches (otherwise from the rest guides skinned to the pose), under the body's interpolated
/// transform.
#[derive(Component, Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HairGuidesPosed {
    pub version: u32,
    pub points: Vec<Vec3>,
}

impl HairGuidesPosed {
    /// Guide `i`'s points, when these match `guides`.
    pub fn guide<'a>(&'a self, guides: &HairGuides, i: usize) -> Option<&'a [Vec3]> {
        if self.version != guides.version || self.points.len() != guides.point_count() {
            return None;
        }
        let start: usize = guides.guides.get(..i)?.iter().map(|g| g.points.len()).sum();
        let len = guides.guides.get(i)?.points.len();
        self.points.get(start..start + len)
    }
}

pub(crate) fn register(app: &mut App) {
    app.register_component::<Appearance>(
        "A person's look: sex, age (years), body shape, face sliders, skin, eyes, hair, wear, \
         headgear.",
    )
    .register_component::<AppearanceRequest>(
        "Ask for a person by seed, family seed, sex and age range (years); the character module \
         picks their Appearance.",
    )
    .register_component::<FaceControls>(
        "Expression this frame: ARKit blendshape weights (0..1) and gaze yaw/pitch (rad).",
    )
    .register_component::<CharacterQuality>(
        "Override how detailed a person is drawn: CloseUp, Mid or Crowd (default: by distance).",
    )
    .register_component::<HairGuides>(
        "Hair guide strands grown on a person (bone indices, model-space rest points, the pose \
         they were grown on, stiffness 0..1), for a hair simulation.",
    )
    .register_component::<HairGuidesPosed>(
        "Simulated hair guide points in the person's model space, in HairGuides order; version \
         echoes the guides simulated.",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appearances_from_data_fill_in_the_rest() {
        let a: Appearance = serde_json::from_str(
            r#"{"age": 61, "hair": {"style": "bun"}, "wear": {"blood": 0.5}}"#,
        )
        .unwrap();
        assert_eq!(a.age, 61.0);
        assert_eq!(a.hair.style, "bun");
        assert_eq!(a.hair.length, 1.0);
        assert_eq!(a.wear.blood, 0.5);
        assert_eq!(a.wear.wet, 0.0);
        assert_eq!(a.headgear, "none");
        assert_eq!(a.skin, Skin::default());
        let back: Appearance = serde_json::from_str(&serde_json::to_string(&a).unwrap()).unwrap();
        assert_eq!(back, a);
        let request: AppearanceRequest =
            serde_json::from_str(r#"{"seed": 7, "family": 3, "age": [20, 40]}"#).unwrap();
        assert_eq!(request.seed, 7);
        assert_eq!(request.family, Some(3));
        assert_eq!(request.age, Some([20.0, 40.0]));
        assert_eq!(request.sex, None);
        let quality: CharacterQuality = serde_json::from_str(r#""Crowd""#).unwrap();
        assert_eq!(quality, CharacterQuality::Crowd);
    }

    #[test]
    fn face_controls_by_name_and_index() {
        assert_eq!(ARKIT.len(), 52);
        assert_eq!(ARKIT[arkit::JAW_OPEN], "jawOpen");
        assert_eq!(ARKIT[arkit::BROW_INNER_UP], "browInnerUp");
        assert_eq!(ARKIT[arkit::NOSE_SNEER_RIGHT], "noseSneerRight");
        let mut face = FaceControls::default();
        assert_eq!(face.get("jawOpen"), 0.0);
        assert!(face.set("jawOpen", 0.4));
        assert!(!face.set("jaw_open", 1.0));
        face.set_index(arkit::EYE_BLINK_LEFT, 1.0);
        assert_eq!(face.get("jawOpen"), 0.4);
        assert_eq!(face.get("eyeBlinkLeft"), 1.0);
        assert_eq!(face.weights.len(), 52);
        let short: FaceControls = serde_json::from_str(r#"{"weights": [0.5]}"#).unwrap();
        assert_eq!(short.get("eyeBlinkLeft"), 0.5);
        assert_eq!(short.get("tongueOut"), 0.0);
    }

    #[test]
    fn posed_hair_matches_its_guides_by_version_and_count() {
        let lock = |n: usize, x: f32| HairGuide {
            bones: [4, 3],
            blend: 0.25,
            points: (0..n)
                .map(|k| Vec3::new(x, 1.7 - 0.05 * k as f32, 0.0))
                .collect(),
            normal: Vec3::Y,
            thickness: 0.004,
        };
        let guides = HairGuides {
            version: 3,
            guides: vec![lock(4, 0.0), lock(6, 0.1)],
            grown_on: vec![BonePose::default(); 5],
            stiffness: 0.2,
        };
        assert_eq!(guides.point_count(), 10);
        let mut posed = HairGuidesPosed {
            version: 3,
            points: guides
                .guides
                .iter()
                .flat_map(|g| g.points.clone())
                .collect(),
        };
        assert_eq!(posed.guide(&guides, 1), Some(&guides.guides[1].points[..]));
        assert_eq!(posed.guide(&guides, 2), None);
        // Stale (simulated from older guides) or short: draw from the rest guides instead.
        posed.version = 2;
        assert_eq!(posed.guide(&guides, 0), None);
        posed.version = 3;
        posed.points.pop();
        assert_eq!(posed.guide(&guides, 0), None);
        let back: HairGuides =
            serde_json::from_str(&serde_json::to_string(&guides).unwrap()).unwrap();
        assert_eq!(back, guides);
    }

    #[test]
    fn the_first_drawer_is_kept() {
        let mut app = App::new();
        assert!(app.world.get_resource::<AppearanceBodies>().is_none());
        draws_appearances(&mut app, "character");
        draws_appearances(&mut app, "other");
        assert_eq!(
            app.world.resource::<AppearanceBodies>().drawn_by,
            "character"
        );
    }
}
