//! Marks effects leave on the ground and walls: charred scorches with cooling embers where
//! spells hit, frost that melts, splashed stone that dries, and chips where shots hit stone.
//!
//! Marks are visual only and fade out. Footprints are not marks: the terrain module presses
//! every `GroundContact` into its deformable ground, and burning leaves the fire module's own
//! scorch. [`Marks`] holds them; [`crate::marks_feature::MarksFeature`] draws them as decals
//! projected onto whatever the scene shows inside each mark's box (`shaders/marks.wgsl`).
//! They come from:
//!
//! - effects whose [`EffectDesc::mark`](crate::effect::EffectDesc::mark) names one: the mark
//!   is left where the effect starts, facing along the effect's up axis (fireballs scorch,
//!   frost bolts frost, shots chip stone);
//! - code and AI agents calling [`Marks::add`] or the `vfx.mark` API.

use std::collections::VecDeque;

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_world::WorldShift;

/// What a mark looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkKind {
    /// Charred ground whose embers glow and cool.
    #[default]
    Scorch,
    /// A patch of frost crystals that melts away.
    Frost,
    /// Ground darkened by water, drying from its edges.
    Wet,
    /// A pock with cracks where something hit stone.
    Chip,
}

impl MarkKind {
    /// Shader code.
    pub fn code(self) -> f32 {
        match self {
            MarkKind::Scorch => 0.0,
            MarkKind::Frost => 1.0,
            MarkKind::Wet => 2.0,
            MarkKind::Chip => 3.0,
        }
    }

    /// Marks that darken or shade what is under them (drawn with a multiply blend).
    pub fn shades(self) -> bool {
        !matches!(self, MarkKind::Frost)
    }

    /// Marks that add light or a lit layer of their own (drawn blended over the scene).
    pub fn covers(self) -> bool {
        matches!(self, MarkKind::Frost | MarkKind::Scorch)
    }
}

/// The mark an effect leaves where it starts ([`crate::effect::EffectDesc::mark`]).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MarkDesc {
    pub kind: MarkKind,
    /// Radius at effect scale 1, meters.
    pub radius: f32,
    /// Seconds the mark lasts; it fades over the last third.
    pub life_secs: f32,
    /// How strong it is, 0..1.
    pub strength: f32,
    /// Seconds a scorch's embers glow.
    pub glow_secs: f32,
}

impl Default for MarkDesc {
    fn default() -> Self {
        Self {
            kind: MarkKind::Scorch,
            radius: 1.0,
            life_secs: 60.0,
            strength: 1.0,
            glow_secs: 0.0,
        }
    }
}

/// One mark in the world.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mark {
    pub kind: MarkKind,
    /// Center, on the surface.
    pub position: DVec3,
    /// Surface normal; the mark is projected along it.
    pub normal: Vec3,
    /// The mark's long axis along the surface.
    pub forward: Vec3,
    /// Half width and half length, meters.
    pub half_size: [f32; 2],
    /// 0..1.
    pub strength: f32,
    /// Seconds it lasts; it fades over the last third.
    pub life_secs: f32,
    /// Seconds a scorch's embers glow.
    pub glow_secs: f32,
    /// Varies the pattern.
    pub seed: u32,
    /// Seconds since it was made (world time, so slow motion slows it).
    pub age_secs: f32,
}

impl Mark {
    /// A round mark of `kind` at `position`, facing `normal`.
    pub fn round(kind: MarkKind, position: DVec3, normal: Vec3, radius: f32) -> Self {
        let normal = normal.normalize_or(Vec3::Y);
        Self {
            kind,
            position,
            normal,
            forward: any_tangent(normal),
            half_size: [radius, radius],
            strength: 1.0,
            life_secs: 60.0,
            glow_secs: 0.0,
            seed: 0,
            age_secs: 0.0,
        }
    }

    /// The mark an effect leaves: `desc` sized by the effect's `scale`, facing the effect's up
    /// axis, turned by the effect's rotation.
    pub fn from_effect(desc: &MarkDesc, position: DVec3, rotation: Quat, scale: f32) -> Self {
        let normal = (rotation * Vec3::Y).normalize_or(Vec3::Y);
        let forward = along_surface(rotation * Vec3::NEG_Z, normal);
        let r = desc.radius.max(0.01) * scale.max(0.01);
        Self {
            forward,
            strength: desc.strength.clamp(0.0, 1.0),
            life_secs: desc.life_secs.max(0.1),
            glow_secs: desc.glow_secs.max(0.0),
            ..Self::round(desc.kind, position, normal, r)
        }
    }

    /// Fraction of its life it has lived, 0..1.
    pub fn age01(&self) -> f32 {
        (self.age_secs / self.life_secs.max(1e-3)).clamp(0.0, 1.0)
    }

    /// Unit right axis (across the mark).
    pub fn right(&self) -> Vec3 {
        self.forward.cross(self.normal).normalize_or(Vec3::X)
    }
}

/// `v` projected onto the plane with normal `n`, normalized (any tangent if `v` is along `n`).
pub fn along_surface(v: Vec3, n: Vec3) -> Vec3 {
    (v - n * v.dot(n))
        .try_normalize()
        .unwrap_or_else(|| any_tangent(n))
}

/// A tangent of the plane with normal `n`: -Z (the engine's forward) where it can be.
fn any_tangent(n: Vec3) -> Vec3 {
    let a = if n.z.abs() < 0.9 {
        Vec3::NEG_Z
    } else {
        Vec3::X
    };
    (a - n * a.dot(n)).normalize_or(Vec3::X)
}

/// Every mark in the world, oldest first. Full means the oldest goes.
#[derive(Resource, Debug, Clone)]
pub struct Marks {
    marks: VecDeque<Mark>,
    /// Most marks kept.
    pub max: usize,
    next_seed: u32,
}

impl Default for Marks {
    fn default() -> Self {
        Self {
            marks: VecDeque::new(),
            max: 2048,
            next_seed: 1,
        }
    }
}

impl Marks {
    /// Leave a mark. A zero `seed` gets a fresh one. A mark of the same kind landing almost
    /// exactly on an older one replaces it, so a spot hit again and again (a target wall)
    /// keeps one mark instead of stacking them into a black blot.
    pub fn add(&mut self, mut mark: Mark) {
        if self.max == 0 {
            return;
        }
        let near = 0.35 * mark.half_size[0].min(mark.half_size[1]) as f64;
        if let Some(i) = self.marks.iter().rposition(|m| {
            m.kind == mark.kind
                && m.normal.dot(mark.normal) > 0.7
                && m.position.distance_squared(mark.position) < near * near
        }) {
            self.marks.remove(i);
        }
        if mark.seed == 0 {
            mark.seed = self.next_seed;
            self.next_seed = self
                .next_seed
                .wrapping_mul(747_796_405)
                .wrapping_add(2_891_336_453);
        }
        while self.marks.len() >= self.max {
            self.marks.pop_front();
        }
        self.marks.push_back(mark);
    }

    pub fn iter(&self) -> impl Iterator<Item = &Mark> {
        self.marks.iter()
    }

    pub fn len(&self) -> usize {
        self.marks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.marks.is_empty()
    }

    pub fn clear(&mut self) {
        self.marks.clear();
    }

    /// Age every mark by `dt` seconds and forget the ones that have faded.
    pub fn tick(&mut self, dt: f32) {
        for m in &mut self.marks {
            m.age_secs += dt;
        }
        self.marks.retain(|m| m.age_secs < m.life_secs);
    }

    /// Move every mark into a new reference frame.
    pub fn shift(&mut self, s: &WorldShift) {
        for m in &mut self.marks {
            m.position = s.point(m.position);
            m.normal = s.direction(m.normal);
            m.forward = s.direction(m.forward);
        }
    }
}

/// Age marks with world time.
pub(crate) fn age_marks(time: Res<Time>, mut marks: ResMut<Marks>) {
    marks.tick(time.delta_secs());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mark(age: f32, x: f64) -> Mark {
        Mark {
            age_secs: age,
            life_secs: 10.0,
            ..Mark::round(MarkKind::Wet, DVec3::new(x, 0.0, 0.0), Vec3::Y, 1.0)
        }
    }

    #[test]
    fn marks_age_fade_and_make_room_for_new_ones() {
        let mut marks = Marks {
            max: 3,
            ..Default::default()
        };
        for (i, age) in [0.0, 5.0, 9.5, 1.0].into_iter().enumerate() {
            marks.add(mark(age, i as f64 * 3.0));
        }
        assert_eq!(marks.len(), 3, "the oldest made way");
        assert!(marks.iter().all(|m| m.seed != 0), "seeds are filled in");
        marks.tick(1.0);
        // 5 + 1 and 1 + 1 live on; 9.5 + 1 is past its life.
        assert_eq!(marks.len(), 2);
    }

    #[test]
    fn a_spot_hit_again_keeps_one_mark() {
        let mut marks = Marks::default();
        for _ in 0..5 {
            marks.add(mark(4.0, 0.0));
            marks.add(mark(0.0, 0.1));
        }
        assert_eq!(marks.len(), 1, "the newest replaced the ones under it");
        assert_eq!(marks.iter().next().unwrap().age_secs, 0.0);
        // Another kind, the far side of a thin wall, or a step away all leave their own.
        marks.add(Mark {
            kind: MarkKind::Scorch,
            ..mark(0.0, 0.0)
        });
        marks.add(Mark {
            normal: Vec3::NEG_Y,
            ..mark(0.0, 0.0)
        });
        marks.add(mark(0.0, 1.0));
        assert_eq!(marks.len(), 4);
    }

    #[test]
    fn effect_marks_face_the_effect_and_scale_with_it() {
        let desc = MarkDesc {
            kind: MarkKind::Frost,
            radius: 0.8,
            ..Default::default()
        };
        // An effect turned to face +X (a hit on a wall).
        let rot = Quat::from_rotation_arc(Vec3::Y, Vec3::X);
        let m = Mark::from_effect(&desc, DVec3::ZERO, rot, 2.0);
        assert!(m.normal.distance(Vec3::X) < 1e-5);
        assert!(m.forward.dot(m.normal).abs() < 1e-5);
        assert_eq!(m.half_size, [1.6, 1.6]);
    }
}
