//! Held items fitted to the hands people see.
//!
//! The weapon clips ([`crate::build::weapons`]) and the props ([`crate::props`]) agree on one
//! *standard fist* per hand: a frame in the logic hand bone's frame with its origin at
//! [`GRIP`], the fingers along −Y, the thumb side along −Z and the palm facing [`PALM`]. A
//! held item is placed in that frame, and its handle is a [`Handle`] there.
//!
//! An imported character's visible hand is another skeleton's hand, retargeted
//! ([`crate::retarget`]): it sits at the logic wrist, but its palm, knuckles and fingers are
//! where that model has them. Each [`Retargeter`] measures where the visible fist closes
//! (from the knuckles and the palm, scaled by the hand's size) and turns each visible hand so
//! its palm faces the standard way, and the attachments and two-handed reaches then hold items
//! in the measured fist instead of the standard one ([`fist_correction`]). The fingers close
//! round the handle that is there ([`HeldItems`], [`Retargeter::wrap`]): each segment curls
//! until it touches the handle, as in knowledge-based grasping (Rijpkema and Girard,
//! SIGGRAPH '91), so a thick bow grip and a thin sword grip close differently.

use std::collections::BTreeMap;

use engine_core::prelude::*;
use serde::{Deserialize, Serialize};

pub use crate::build::weapons::GRIP;
use crate::retarget::Retargeter;
use crate::xform::Xf;

/// Which way each standard fist's palm faces in its hand's frame (left, right): toward the
/// body for hands hanging at the sides of a person facing −Z.
pub const PALM: [Vec3; 2] = [Vec3::X, Vec3::NEG_X];

/// The handle radius the standard fist closes round (an arming sword's grip), meters.
pub const STANDARD_RADIUS: f32 = 0.017;

/// Which hand a bone name is (`hand_l` → 0, `hand_r` → 1).
pub fn side_of(bone: &str) -> Option<usize> {
    if bone.ends_with("_l") {
        Some(0)
    } else if bone.ends_with("_r") {
        Some(1)
    } else {
        None
    }
}

/// A held item's handle, in the holding hand's standard fist frame: a cylinder of `radius`
/// along `axis` through the fist, from `extent[0]` to `extent[1]` along it, its axis moved
/// `radius − STANDARD_RADIUS` further from the palm so a thicker handle still rests on it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Handle {
    pub axis: Vec3,
    pub radius: f32,
    pub extent: [f32; 2],
}

impl Default for Handle {
    fn default() -> Self {
        Self {
            axis: Vec3::NEG_Z,
            radius: STANDARD_RADIUS,
            extent: [-0.06, 0.06],
        }
    }
}

impl Handle {
    pub fn new(axis: Vec3, radius: f32, extent: [f32; 2]) -> Self {
        Self {
            axis: axis.normalize_or(Vec3::NEG_Z),
            radius,
            extent,
        }
    }

    /// The handle's axis end points in `side`'s standard fist frame, hand-bone space (the
    /// fist at [`GRIP`]).
    pub fn ends(&self, side: usize) -> [Vec3; 2] {
        let centre = GRIP + PALM[side] * (self.radius - STANDARD_RADIUS);
        self.extent.map(|d| centre + self.axis * d)
    }
}

/// How a held item sits in its fist besides its handle: slid `slide` meters along the handle
/// and rolled `roll` degrees about it (a sword's edge turned in the hand). The hand stays where
/// it is on the handle; the item moves.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Seat {
    pub slide: f32,
    pub roll: f32,
}

impl Seat {
    /// The seat as a change of the item's placement in `side`'s standard fist frame (an item
    /// at `offset` for the plain fist is at `seat ∘ offset`).
    pub fn xf(&self, handle: &Handle, side: usize) -> Xf {
        if *self == Seat::default() {
            return Xf::IDENTITY;
        }
        let [a, b] = handle.ends(side);
        let centre = (a + b) * 0.5;
        let roll = Quat::from_axis_angle(handle.axis, self.roll.to_radians());
        Xf::from_tr(centre + handle.axis * self.slide, roll).mul(&Xf::from_t(-centre))
    }
}

/// What each hand of a character holds (left, right), for the fingers to close round.
/// Props add it ([`crate::props`]); a hand with nothing closes as its curves say.
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HeldItems {
    pub hands: [Option<Handle>; 2],
    /// How each hand's item sits in it ([`Seat`]).
    pub seats: [Seat; 2],
    /// Extra curl of each finger (thumb first) past where it touches the handle, degrees.
    pub trims: [[f32; 5]; 2],
    /// The [`GripPresets`] entry each hand's grip came from (empty for none).
    pub presets: [String; 2],
}

impl HeldItems {
    pub fn hold(mut self, side: usize, handle: Handle) -> Self {
        self.hands[side] = Some(handle);
        self
    }

    /// Hold `handle` in `side` as preset `name` says (its seat and trims).
    pub fn hold_preset(
        mut self,
        side: usize,
        handle: Handle,
        name: &str,
        preset: &GripPreset,
    ) -> Self {
        self.hands[side] = Some(handle);
        self.seats[side] = preset.seat;
        self.trims[side] = preset.trims;
        self.presets[side] = name.to_string();
        self
    }
}

/// One held item's grip, as data: its handle's size (what the item's mesh is built round) and
/// how it sits in the hand. The editor's Grips tab tunes these and saves them to
/// `assets/anim/grips.json`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GripPreset {
    pub radius: f32,
    pub extent: [f32; 2],
    #[serde(flatten)]
    pub seat: Seat,
    pub trims: [f32; 5],
}

impl Default for GripPreset {
    fn default() -> Self {
        Self {
            radius: STANDARD_RADIUS,
            extent: [-0.06, 0.06],
            seat: Seat::default(),
            trims: [0.0; 5],
        }
    }
}

impl GripPreset {
    /// This grip's handle along `axis`, its extent moved `shift` along it.
    pub fn handle(&self, axis: Vec3, shift: f32) -> Handle {
        Handle::new(axis, self.radius, self.extent.map(|e| e + shift))
    }
}

/// The grips of the held items, by name (`sword`, `shield`, `greatsword`, `bow`, ...). The
/// built-in set is `assets/anim/grips.json`; changing an entry re-seats and re-curls every
/// hand holding it ([`HeldItems::presets`]).
#[derive(Resource, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GripPresets(pub BTreeMap<String, GripPreset>);

/// The built-in grips file, relative to the repository's asset folder.
pub const GRIPS_FILE: &str = "anim/grips.json";

impl Default for GripPresets {
    fn default() -> Self {
        Self::builtin()
    }
}

impl GripPresets {
    /// The grips this build was made with (`assets/anim/grips.json`).
    pub fn builtin() -> Self {
        Self::parse(include_str!("../../../assets/anim/grips.json")).unwrap_or_else(|e| {
            tracing::warn!("grips.json: {e}");
            Self(BTreeMap::new())
        })
    }

    pub fn parse(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json).map(Self)
    }

    /// The file's text: one entry per item, sorted by name.
    pub fn to_json(&self) -> String {
        let mut out = serde_json::to_string_pretty(&self.0).unwrap_or_default();
        out.push('\n');
        out
    }

    /// Item `name`'s grip, or the standard one.
    pub fn get(&self, name: &str) -> GripPreset {
        self.0.get(name).copied().unwrap_or_default()
    }
}

/// Re-seats and re-curls held items when their presets change (the editor tunes them live).
pub(crate) fn apply_grip_presets(presets: Option<Res<GripPresets>>, mut q: Query<&mut HeldItems>) {
    let Some(presets) = presets else {
        return;
    };
    if !presets.is_changed() {
        return;
    }
    for mut held in &mut q {
        for side in 0..2 {
            let Some(preset) = presets.0.get(&held.presets[side]).copied() else {
                continue;
            };
            if held.seats[side] != preset.seat || held.trims[side] != preset.trims {
                held.seats[side] = preset.seat;
                held.trims[side] = preset.trims;
            }
        }
    }
}

/// The change from the standard fist to the measured visible one of `side`, in the hand bone's
/// frame: an item placed at `bone ∘ offset` for the standard fist belongs at
/// `bone ∘ correction ∘ offset`. Identity without a retargeted model.
pub fn fist_correction(retargeter: Option<&Retargeter>, side: usize) -> Xf {
    match retargeter.and_then(|r| r.fist(side)) {
        Some(fist) => fist.mul(&Xf::from_tr(-GRIP, Quat::IDENTITY)),
        None => Xf::IDENTITY,
    }
}

/// How one visible hand sits on what it holds ([`Retargeter::grip_report`]), meters: a gap
/// is positive, a penetration negative.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct GripReport {
    /// The palm's skin under the knuckles to the handle's surface.
    pub palm: f32,
    /// The deepest any finger segment sinks into the handle (0 if none does).
    pub deepest: f32,
    /// The widest gap between a finger (past the thumb) and the handle, over each finger's
    /// closest segment.
    pub widest: f32,
    /// Whether the thumb closes on the far side of the handle from the palm.
    pub thumb_opposed: bool,
    /// The handle's diameter over the hand's length (comfortable near 0.2).
    pub size: f32,
}

/// What a [`GripReport`] measured, in the character's model space
/// ([`Retargeter::grip_geometry`]).
#[derive(Debug, Clone, PartialEq)]
pub struct GripGeometry {
    pub report: GripReport,
    /// The handle's axis end points and radius.
    pub handle: [Vec3; 2],
    pub radius: f32,
    /// The palm's skin under the knuckles, and which way the palm faces.
    pub palm: Vec3,
    pub palm_dir: Vec3,
    /// Each finger's segments (thumb first, knuckle out): start, end, and the gap to the
    /// handle's surface (negative inside it).
    pub fingers: Vec<Vec<(Vec3, Vec3, f32)>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_thicker_handle_sits_further_from_the_palm() {
        let thin = Handle::new(Vec3::NEG_Z, STANDARD_RADIUS, [0.0, 0.0]);
        let thick = Handle::new(Vec3::NEG_Z, 0.02, [0.0, 0.0]);
        assert_eq!(thin.ends(1)[0], GRIP);
        // The right palm faces −X: the thick grip's axis moves 3 mm further that way.
        assert!((thick.ends(1)[0] - GRIP + Vec3::X * 0.003).length() < 1e-6);
        assert!((thick.ends(0)[0] - GRIP - Vec3::X * 0.003).length() < 1e-6);
    }

    #[test]
    fn a_seat_slides_and_rolls_the_item_about_its_handle() {
        let handle = Handle::new(Vec3::NEG_Z, 0.02, [-0.05, 0.05]);
        let centre = handle.ends(1)[0].lerp(handle.ends(1)[1], 0.5);
        let rolled = Seat {
            slide: 0.0,
            roll: 90.0,
        }
        .xf(&handle, 1);
        // The handle's axis stays put; a point off it turns about it.
        assert!(rolled.transform_point(centre).distance(centre) < 1e-6);
        let edge = centre + Vec3::Y * 0.1;
        let turned = rolled.transform_point(edge) - centre;
        assert!(turned.dot(Vec3::Y).abs() < 1e-5 && turned.length() > 0.099);
        let slid = Seat {
            slide: 0.03,
            roll: 0.0,
        }
        .xf(&handle, 1);
        assert!((slid.transform_point(edge) - edge - Vec3::NEG_Z * 0.03).length() < 1e-6);
        assert_eq!(Seat::default().xf(&handle, 1), Xf::IDENTITY);
    }

    #[test]
    fn without_a_model_the_standard_fist_holds() {
        assert_eq!(fist_correction(None, 0), Xf::IDENTITY);
        assert_eq!(side_of("hand_l"), Some(0));
        assert_eq!(side_of("hand_r"), Some(1));
        assert_eq!(side_of("head"), None);
    }
}
