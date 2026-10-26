//! Detail by distance, chosen every frame without rebuilding: far from the camera a person
//! draws the base mesh's own quads instead of the subdivided skin, and fewer, wider hairs
//! with fewer points each, so a crowd costs a fraction of a close-up while a hairdo keeps its
//! coverage and colour.

use engine_core::prelude::*;

use crate::strands::StrandKind;

/// Scales the distance the detail is chosen by (0 keeps every person at full detail, as the
/// creator's portraits do; 2 drops detail twice as near).
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct CharacterLod {
    pub distance_scale: f32,
}

impl Default for CharacterLod {
    fn default() -> Self {
        Self {
            distance_scale: 1.0,
        }
    }
}

/// Beyond this distance (m) the skin draws the base mesh's quads.
pub const COARSE_SKIN: f32 = 5.0;
/// Long hair is drawn in full up to here (m); beyond, the share of hairs drawn falls with the
/// square of the distance (as the area they cover on screen does).
pub const FULL_HAIR: f32 = 1.5;
/// Short hair and beards in full up to here: their hairs are cheap (a few segments each), and
/// thinned out they turn to specks, since a short hair drawn wider stays short.
pub const FULL_SHORT_HAIR: f32 = 3.0;
/// Brows and lashes in full up to here, none beyond `FINE_HAIR_END`.
pub const FULL_FINE_HAIR: f32 = 3.0;
pub const FINE_HAIR_END: f32 = 10.0;
/// No strands at all beyond this distance.
pub const HAIR_END: f32 = 60.0;
/// The fewest hairs a set keeps (as a share of all of them).
const MIN_SHARE: f32 = 0.015;

/// How to draw a strand set at `distance` m: hairs drawn (the first ones: hairs are grown in
/// random order over the scalp, so any first share covers it evenly), points stepped over per
/// segment, and how much wider each hair is drawn so the set covers as much as all of them.
pub fn strands(kind: StrandKind, hairs: u32, points: u32, distance: f32) -> (u32, u32, f32) {
    let fine = matches!(kind, StrandKind::Brows | StrandKind::Lashes);
    if distance > HAIR_END || (fine && distance > FINE_HAIR_END) || hairs == 0 {
        return (0, 1, 1.0);
    }
    let full = if fine {
        FULL_FINE_HAIR
    } else if kind == StrandKind::Beard || points <= 8 {
        FULL_SHORT_HAIR
    } else {
        FULL_HAIR
    };
    let share = (full / distance.max(1e-3)).powi(2).clamp(MIN_SHARE, 1.0);
    let drawn = ((hairs as f32 * share).ceil() as u32).clamp(1, hairs);
    let segments = points.saturating_sub(1).max(1);
    let stride = if distance < 4.0 {
        1
    } else if distance < 10.0 {
        2
    } else {
        4
    };
    // At least two segments a hair.
    let stride = stride.min((segments / 2).max(1));
    (drawn, stride, hairs as f32 / drawn as f32)
}

/// Vertices to draw per hair of `points` points stepping `stride` points per segment.
pub fn strand_vertices(points: u32, stride: u32) -> u32 {
    points.saturating_sub(1).div_ceil(stride.max(1)) * 6
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_up_is_full_detail_and_far_hair_keeps_its_coverage() {
        assert_eq!(
            strands(StrandKind::Scalp, 50_000, 32, 0.8),
            (50_000, 1, 1.0)
        );
        let (drawn, stride, width) = strands(StrandKind::Scalp, 50_000, 32, 10.0);
        assert!(drawn < 2_000 && stride == 4, "{drawn} {stride}");
        assert!((drawn as f32 * width - 50_000.0).abs() < 1.0);
        // Short hair keeps two segments.
        assert_eq!(strands(StrandKind::Scalp, 1_000, 5, 30.0).1, 2);
        assert_eq!(strands(StrandKind::Lashes, 380, 6, 12.0).0, 0);
        assert_eq!(strands(StrandKind::Beard, 30_000, 6, 70.0).0, 0);
        assert_eq!(strand_vertices(32, 1), 31 * 6);
        assert_eq!(strand_vertices(32, 4), 8 * 6);
    }
}
