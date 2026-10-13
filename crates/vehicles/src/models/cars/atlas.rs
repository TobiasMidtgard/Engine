//! The body texture atlas: four planar projections of the body (side, top, front, back) laid
//! out on one square texture. Each body triangle picks the projection its normal faces most
//! (the mesher's UV mode), so shut lines, stripes and lettering drawn into the atlas land on
//! the body undistorted. Both sides share the side view, or with [`Atlas::split_sides`] each
//! side gets its own (for liveries with lettering that must read right on both sides).

use engine_core::math::{Vec2, Vec3};

use super::tex::Canvas;

/// Which projection a body triangle uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Side = 0,
    Top = 1,
    Front = 2,
    Back = 3,
    Bottom = 4,
}

impl View {
    pub fn of(n: Vec3) -> View {
        let a = n.abs();
        if a.x >= a.y && a.x >= a.z {
            View::Side
        } else if a.y >= a.z {
            if n.y >= 0.0 { View::Top } else { View::Bottom }
        } else if n.z < 0.0 {
            View::Front
        } else {
            View::Back
        }
    }

    pub fn from_mode(m: u8) -> View {
        match m {
            0 => View::Side,
            1 => View::Top,
            2 => View::Front,
            3 => View::Back,
            _ => View::Bottom,
        }
    }
}

/// Atlas layout for a body spanning `z0..z1` (length), `|x| ≤ half_w` and heights `0..top`
/// above the ground.
#[derive(Debug, Clone, Copy)]
pub struct Atlas {
    pub size: u32,
    pub z0: f32,
    pub z1: f32,
    pub half_w: f32,
    pub top: f32,
    /// Ground height below the body origin.
    pub ground: f32,
    /// Pixels per meter.
    pub ppm: f32,
    /// Separate side views for the right (x > 0) and left sides.
    pub split: bool,
}

const GAP: f32 = 4.0;

impl Atlas {
    pub fn new(size: u32, z0: f32, z1: f32, half_w: f32, top: f32, ground: f32) -> Self {
        Self::layout(size, z0, z1, half_w, top, ground, false)
    }

    /// Like [`Atlas::new`] with separate right and left side views.
    pub fn split_sides(size: u32, z0: f32, z1: f32, half_w: f32, top: f32, ground: f32) -> Self {
        Self::layout(size, z0, z1, half_w, top, ground, true)
    }

    fn layout(
        size: u32,
        z0: f32,
        z1: f32,
        half_w: f32,
        top: f32,
        ground: f32,
        split: bool,
    ) -> Self {
        // Height needed: side view(s) (top each) + top view (2 half_w) + front/back (top),
        // plus gaps.
        let s = size as f32;
        let sides = if split { 2.0 } else { 1.0 };
        let ppm = ((s - (4.0 + sides) * GAP) / ((1.0 + sides) * top + 2.0 * half_w))
            .min((s - 2.0 * GAP) / (z1 - z0));
        let ppm = ppm.min((s - 3.0 * GAP) / (4.0 * half_w));
        Self {
            size,
            z0,
            z1,
            half_w,
            top,
            ground,
            ppm,
            split,
        }
    }

    /// First pixel rows of the right side view, the left side view (the same row unless
    /// split), the top view and the front/back views.
    fn rows(&self) -> (f32, f32, f32, f32) {
        let right = GAP;
        let left = if self.split {
            right + self.top * self.ppm + GAP
        } else {
            right
        };
        let top = left + self.top * self.ppm + GAP;
        let ends = top + 2.0 * self.half_w * self.ppm + GAP;
        (right, left, top, ends)
    }

    /// Pixel position of body point `p` in projection `view`.
    pub fn pixel(&self, view: View, p: Vec3) -> Vec2 {
        let (r_right, r_left, r_top, r_ends) = self.rows();
        let h = p.y + self.ground;
        let k = self.ppm;
        let r_side = if p.x < 0.0 { r_left } else { r_right };
        match view {
            View::Side => Vec2::new(GAP + (p.z - self.z0) * k, r_side + (self.top - h) * k),
            View::Top => Vec2::new(GAP + (p.z - self.z0) * k, r_top + (p.x + self.half_w) * k),
            View::Front => Vec2::new(GAP + (self.half_w - p.x) * k, r_ends + (self.top - h) * k),
            View::Back => Vec2::new(
                2.0 * GAP + 2.0 * self.half_w * k + (p.x + self.half_w) * k,
                r_ends + (self.top - h) * k,
            ),
            View::Bottom => Vec2::splat(self.size as f32 - 2.0),
        }
    }

    /// Texture coordinate of `p` in `view`.
    pub fn uv(&self, view: View, p: Vec3) -> Vec2 {
        self.pixel(view, p) / self.size as f32
    }

    /// Inverse of [`Atlas::pixel`] for the planar coordinates a view keeps: side → (z, h),
    /// top → (z, x), front/back → (x, h); the flag marks the left side view of a split atlas.
    /// `None` outside every region.
    pub fn region_at(&self, px: Vec2) -> Option<(View, Vec2, bool)> {
        let (r_right, r_left, r_top, r_ends) = self.rows();
        let k = self.ppm;
        let z = self.z0 + (px.x - GAP) / k;
        let in_z = (self.z0..=self.z1).contains(&z);
        let side_h = self.top * k;
        if px.y >= r_right && px.y < r_right + side_h && in_z {
            return Some((
                View::Side,
                Vec2::new(z, self.top - (px.y - r_right) / k),
                false,
            ));
        }
        if self.split && px.y >= r_left && px.y < r_left + side_h && in_z {
            return Some((
                View::Side,
                Vec2::new(z, self.top - (px.y - r_left) / k),
                true,
            ));
        }
        if px.y >= r_top && px.y < r_ends - GAP && in_z {
            return Some((
                View::Top,
                Vec2::new(z, (px.y - r_top) / k - self.half_w),
                false,
            ));
        }
        if px.y >= r_ends && px.y < r_ends + self.top * k {
            let h = self.top - (px.y - r_ends) / k;
            let x_front = self.half_w - (px.x - GAP) / k;
            if x_front.abs() <= self.half_w {
                return Some((View::Front, Vec2::new(x_front, h), false));
            }
            let x_back = (px.x - 2.0 * GAP - 2.0 * self.half_w * k) / k - self.half_w;
            if x_back.abs() <= self.half_w {
                return Some((View::Back, Vec2::new(x_back, h), false));
            }
        }
        None
    }

    /// Stamp a 3D polyline (points with their surface normals) into the canvas: each segment
    /// goes into the views its normal faces or nearly faces (so lines stay continuous where
    /// neighbouring triangles switch projection). Width `reach` in pixels.
    pub fn stamp(&self, canvas: &mut Canvas, pts: &[(Vec3, Vec3)], reach: f32, tag: u8) {
        for w in pts.windows(2) {
            let n = (w[0].1 + w[1].1).normalize_or(Vec3::Y);
            for view in [View::Side, View::Top, View::Front, View::Back] {
                if faces(view, n) {
                    let a = self.pixel(view, w[0].0);
                    let b = self.pixel(view, w[1].0);
                    canvas.segment(a, b, reach, tag);
                }
            }
        }
    }
}

/// Whether normal `n` faces `view` closely enough for lines to be drawn there (the views
/// overlap by about 15° around each switch).
fn faces(view: View, n: Vec3) -> bool {
    let a = n.abs();
    let m = 0.75;
    match view {
        View::Side => a.x >= m * a.y.max(a.z),
        View::Top => n.y > 0.0 && a.y >= m * a.x.max(a.z),
        View::Front => n.z < 0.0 && a.z >= m * a.x.max(a.y),
        View::Back => n.z > 0.0 && a.z >= m * a.x.max(a.y),
        View::Bottom => false,
    }
}
