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

/// What each hand of a character holds (left, right), for the fingers to close round.
/// Props add it ([`crate::props`]); a hand with nothing closes as its curves say.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HeldItems {
    pub hands: [Option<Handle>; 2],
}

impl HeldItems {
    pub fn hold(mut self, side: usize, handle: Handle) -> Self {
        self.hands[side] = Some(handle);
        self
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
    fn without_a_model_the_standard_fist_holds() {
        assert_eq!(fist_correction(None, 0), Xf::IDENTITY);
        assert_eq!(side_of("hand_l"), Some(0));
        assert_eq!(side_of("hand_r"), Some(1));
        assert_eq!(side_of("head"), None);
    }
}
