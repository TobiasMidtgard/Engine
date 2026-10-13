//! Glass openings: an outline in one projection plane (plan view for windshields, side view
//! for side windows, rear view for upright hatches), the black frit band width, and a
//! parametrization between two edges used to drape the pane onto the body.

use engine_core::math::{Vec2, Vec3};

use super::details::Proj;
use super::sdf::sd_polygon;

pub struct Pane {
    pub outline: Vec<Vec2>,
    pub frit: f32,
    /// Plane of the outline: `Top` (x, z), `Side` (z, h) or `Back` (x, h).
    pub view: Proj,
    /// Ground height below the body origin (heights in the outline are above the ground).
    pub ground: f32,
    lo: Vec<Vec2>,
    hi: Vec<Vec2>,
    bbox: (Vec2, Vec2),
}

impl Pane {
    /// A pane between edge `lo` and edge `hi` (same point count, matching order).
    pub fn from_edges(lo: Vec<Vec2>, hi: Vec<Vec2>, frit: f32, view: Proj, ground: f32) -> Self {
        let mut outline = lo.clone();
        outline.extend(hi.iter().rev());
        let bbox = outline.iter().fold(
            (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)),
            |(a, b), p| (a.min(*p), b.max(*p)),
        );
        Self {
            outline,
            frit,
            view,
            ground,
            lo,
            hi,
            bbox,
        }
    }

    /// Point at (s along the edges, t from the `lo` edge to the `hi` edge).
    pub fn point(&self, s: f32, t: f32) -> Vec2 {
        let n = self.lo.len();
        let f = s.clamp(0.0, 1.0) * (n - 1) as f32;
        let i = (f as usize).min(n - 2);
        let u = f - i as f32;
        let a = self.lo[i].lerp(self.lo[i + 1], u);
        let b = self.hi[i].lerp(self.hi[i + 1], u);
        a.lerp(b, t)
    }

    /// The projection of a body point into this pane's plane.
    pub fn project(&self, p: Vec3) -> Vec2 {
        match self.view {
            Proj::Top => Vec2::new(p.x, p.z),
            Proj::Side => Vec2::new(p.z, p.y + self.ground),
            Proj::Front | Proj::Back => Vec2::new(p.x, p.y + self.ground),
        }
    }

    /// Signed distance to the outline in the pane's plane (negative inside). Far outside the
    /// outline's box only a lower bound (the distance to the box).
    pub fn sd(&self, q: Vec2) -> f32 {
        let (lo, hi) = self.bbox;
        let out = (lo - q).max(q - hi).max(Vec2::ZERO).length();
        if out > 0.05 {
            return out;
        }
        sd_polygon(q, &self.outline)
    }

    /// Signed distance of a body point's projection (side panes on either side).
    pub fn sd_at(&self, p: Vec3) -> f32 {
        self.sd(self.project(p))
    }

    /// Ray for draping at (s, t): origin outside the body and direction.
    pub fn ray(&self, s: f32, t: f32) -> (Vec3, Vec3) {
        self.view.ray(self.point(s, t), self.ground)
    }
}
