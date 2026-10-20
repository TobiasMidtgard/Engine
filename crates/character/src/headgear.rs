//! What covers the head (`Appearance::headgear`): the parts of the person a helmet hides are
//! left out, so whoever draws the helmet (an equipment or NPC module) draws it over a head that
//! doesn't poke through. `open_helm` hides the hair, ears and neck and keeps the face from brow
//! to chin; `closed_helm` hides the whole head and neck. `hood` and unknown ids hide nothing yet.

use engine_core::math::Vec3;

use crate::mesh::SkinMesh;
use crate::strands::{StrandKind, StrandSet};

/// Rest height above which the neck and head start (the shirt's collar sits below it).
const NECK_BASE: f32 = 0.55;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Headgear {
    None,
    OpenHelm,
    ClosedHelm,
}

impl Headgear {
    pub fn from_id(id: &str) -> Self {
        match id {
            "open_helm" => Headgear::OpenHelm,
            "closed_helm" => Headgear::ClosedHelm,
            _ => Headgear::None,
        }
    }

    /// Whether a rest-space point is under the headgear. `head`: the rest head center (between
    /// the ears, level with the eyes).
    pub fn hides(self, rest: Vec3, head: Vec3) -> bool {
        if self == Headgear::None || rest.y < NECK_BASE {
            return false;
        }
        if self == Headgear::ClosedHelm {
            return true;
        }
        // The face an open helm leaves bare: in front of the ears, from the brows to the chin.
        let d = rest - head;
        let face = d.z > 0.03 && d.x.abs() < 0.064 && d.y < 0.045 && d.y > -0.14;
        !face
    }

    /// Whether any of the eyes, mouth, brows and lashes show.
    pub fn face_shows(self) -> bool {
        self != Headgear::ClosedHelm
    }

    /// Leave out the skin (and anything anchored to it, such as teeth) under the headgear.
    pub fn cover_skin(self, skin: &mut SkinMesh, head: Vec3) {
        if self == Headgear::None {
            return;
        }
        let hidden: Vec<bool> = skin.rest.iter().map(|r| self.hides(*r, head)).collect();
        let kept: Vec<u32> = skin
            .indices
            .as_chunks::<3>()
            .0
            .iter()
            .filter(|t| t.iter().all(|v| !hidden[*v as usize]))
            .flatten()
            .copied()
            .collect();
        skin.indices = kept;
    }

    /// Leave out the hair under the headgear.
    pub fn cover_hair(self, strands: &mut Vec<StrandSet>) {
        match self {
            Headgear::None => {}
            Headgear::OpenHelm => strands.retain(|s| s.kind != StrandKind::Scalp),
            Headgear::ClosedHelm => strands.clear(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_open_helm_keeps_the_face_and_a_closed_one_hides_the_head() {
        let head = Vec3::new(0.0, 0.675, 0.02);
        let nose = Vec3::new(0.0, 0.64, 0.12);
        let crown = Vec3::new(0.0, 0.78, 0.0);
        let ear = Vec3::new(0.075, 0.67, 0.0);
        let neck_back = Vec3::new(0.0, 0.58, -0.04);
        let chest = Vec3::new(0.0, 0.4, 0.08);
        let open = Headgear::from_id("open_helm");
        assert!(!open.hides(nose, head));
        assert!(open.hides(crown, head) && open.hides(ear, head) && open.hides(neck_back, head));
        assert!(!open.hides(chest, head));
        let closed = Headgear::from_id("closed_helm");
        assert!(closed.hides(nose, head) && !closed.hides(chest, head));
        assert!(!closed.face_shows());
        let none = Headgear::from_id("feathered_cap");
        assert!(!none.hides(crown, head));
    }
}
