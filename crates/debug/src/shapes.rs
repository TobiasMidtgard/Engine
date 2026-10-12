//! Shapes: the immediate-mode methods on [`DebugDraw`] and the serializable [`DebugShape`]
//! used by the `debug.draw` API.
//!
//! Every shape is tessellated into line segments right away, in `f64` world space. Sizes are
//! meters, angles radians.

use engine_core::math::{DMat3, DVec3, DVec4, Mat4, Quat, Vec3, Vec4};
use engine_core::transform::{GlobalTransform, Transform};
use engine_render::{Camera, Projection};
use serde::{Deserialize, Serialize};

use crate::color::{AXIS_X, AXIS_Y, AXIS_Z, Color};
use crate::draw::{DebugDraw, DepthMode, ShapeHandle};

/// Size of the cross drawn by [`DebugDraw::point`], meters.
pub const POINT_SIZE: f32 = 0.15;

/// Largest `cells` accepted by [`DebugDraw::grid`] (keeps a typo from drawing a million lines).
pub const MAX_GRID_CELLS: u32 = 1000;

/// A shape as data: what `debug.draw` accepts, and what [`DebugDraw::shape`] draws.
///
/// JSON uses the snake_case variant name as the key:
/// `{"sphere": {"center": [0, 1, 0], "radius": 0.5}}`. Positions are world meters (`f64`),
/// directions and sizes `f32`, rotations quaternions `[x, y, z, w]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DebugShape {
    /// Segment from `a` to `b`.
    Line { a: DVec3, b: DVec3 },
    /// Segment from `origin` to `origin + dir` (the length of `dir` is the ray length).
    Ray { origin: DVec3, dir: Vec3 },
    /// Arrow from `from` to `to`. `head` is the head length in meters (0 = automatic).
    Arrow {
        from: DVec3,
        to: DVec3,
        #[serde(default)]
        head: f32,
    },
    /// Axis-aligned box between two corners.
    Aabb { min: DVec3, max: DVec3 },
    /// Oriented box: `center`, `half_extents` along its local axes, `rotation`.
    #[serde(rename = "box", alias = "oriented_box", alias = "obb")]
    OrientedBox {
        center: DVec3,
        half_extents: Vec3,
        #[serde(default)]
        rotation: Quat,
    },
    /// Three great circles (around X, Y and Z).
    Sphere { center: DVec3, radius: f32 },
    /// Circle in the plane through `center` perpendicular to `normal` (default +Y).
    Circle {
        center: DVec3,
        #[serde(default = "up")]
        normal: Vec3,
        radius: f32,
    },
    /// Three short axis-aligned lines through `at`, `size` meters long (default 0.15).
    #[serde(alias = "point")]
    Cross {
        at: DVec3,
        #[serde(default = "point_size")]
        size: f32,
    },
    /// Coordinate axes (X red, Y green, Z blue) of a frame, `size` meters long (default 1).
    /// The color of the shape is ignored.
    Axes {
        position: DVec3,
        #[serde(default)]
        rotation: Quat,
        #[serde(default = "one")]
        size: f32,
    },
    /// Capsule around the segment `a`–`b`.
    Capsule { a: DVec3, b: DVec3, radius: f32 },
    /// Cylinder around the segment `a`–`b` (flat caps).
    Cylinder { a: DVec3, b: DVec3, radius: f32 },
    /// Square grid of `cells` × `cells` cells of `cell` meters, centered on `center`, in the
    /// plane perpendicular to `normal` (default +Y: a ground grid).
    Grid {
        center: DVec3,
        #[serde(default = "up")]
        normal: Vec3,
        #[serde(default = "one")]
        cell: f32,
        #[serde(default = "ten")]
        cells: u32,
    },
    /// Frustum of a camera-relative view-projection matrix (columns, wgpu clip space) whose
    /// camera sits at `origin`. Infinite far planes are cut at `max_distance` meters.
    Frustum {
        view_proj: Mat4,
        #[serde(default)]
        origin: DVec3,
        #[serde(default = "hundred")]
        max_distance: f32,
    },
    /// Polyline through `points`; `closed` joins the last point to the first.
    #[serde(alias = "polyline")]
    Path {
        points: Vec<DVec3>,
        #[serde(default)]
        closed: bool,
    },
}

fn up() -> Vec3 {
    Vec3::Y
}
fn one() -> f32 {
    1.0
}
fn ten() -> u32 {
    10
}
fn hundred() -> f32 {
    100.0
}
fn point_size() -> f32 {
    POINT_SIZE
}

impl DebugShape {
    /// JSON keys accepted for shapes (`point` is a small `cross`; `obb`, `oriented_box` and
    /// `polyline` are also accepted as aliases of `box` and `path`).
    pub const KINDS: [&'static str; 15] = [
        "line", "ray", "arrow", "aabb", "box", "sphere", "circle", "cross", "point", "axes",
        "capsule", "cylinder", "grid", "frustum", "path",
    ];
}

/// Color, lifetime and depth mode of a shape (for [`DebugDraw::styled`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapeStyle {
    /// Linear RGBA color (ignored by `axes`, which use the axis colors).
    pub color: Color,
    /// Real seconds; 0 = one frame, [`DebugDraw::FOREVER`] = until cleared.
    pub duration: f32,
    /// Depth-tested or always on top.
    pub depth: DepthMode,
}

impl Default for ShapeStyle {
    fn default() -> Self {
        Self {
            color: crate::color::YELLOW,
            duration: 0.0,
            depth: DepthMode::Tested,
        }
    }
}

/// Two unit vectors perpendicular to `n` and to each other (any `n`, even zero-length).
fn basis(n: DVec3) -> (DVec3, DVec3, DVec3) {
    let n = n.try_normalize().unwrap_or(DVec3::Y);
    let (u, v) = n.any_orthonormal_pair();
    (u, v, n)
}

/// Rotate `q` (f32) as an f64 matrix.
fn dmat(q: Quat) -> DMat3 {
    DMat3::from_quat(q.as_dquat().normalize())
}

impl DebugDraw {
    fn segment_count(&self) -> u32 {
        self.segments.clamp(4, 256)
    }

    /// Polyline `center + u·cos(t) + v·sin(t)` for `t` in `angles` (radians; `u`, `v` carry
    /// the radius).
    fn arc(
        &mut self,
        center: DVec3,
        (u, v): (DVec3, DVec3),
        angles: std::ops::Range<f64>,
        segments: u32,
        color: Color,
    ) {
        let (t0, t1) = (angles.start, angles.end);
        let point = |t: f64| {
            let (s, c) = t.sin_cos();
            center + u * c + v * s
        };
        let mut prev = point(t0);
        for i in 1..=segments {
            let next = point(t0 + (t1 - t0) * i as f64 / segments as f64);
            self.push(prev, next, color);
            prev = next;
        }
    }

    fn ring(&mut self, center: DVec3, u: DVec3, v: DVec3, color: Color) {
        let n = self.segment_count();
        self.arc(center, (u, v), 0.0..std::f64::consts::TAU, n, color);
    }

    fn box_edges(&mut self, corner: impl Fn(f64, f64, f64) -> DVec3, color: Color) {
        let c = |i: u32| {
            let s = |bit: u32| if i & bit != 0 { 1.0 } else { -1.0 };
            corner(s(1), s(2), s(4))
        };
        for i in 0..8u32 {
            for bit in [1u32, 2, 4] {
                if i & bit == 0 {
                    self.push(c(i), c(i | bit), color);
                }
            }
        }
    }

    // ------------------------------------------------------------------ shapes

    /// A segment from `a` to `b`.
    pub fn line(&mut self, a: DVec3, b: DVec3, color: Color) -> ShapeHandle<'_> {
        let start = self.begin_shape();
        self.push(a, b, color);
        self.finish_shape(start)
    }

    /// A segment from `origin` along `dir` (its length is the ray length, meters).
    pub fn ray(&mut self, origin: DVec3, dir: Vec3, color: Color) -> ShapeHandle<'_> {
        self.line(origin, origin + dir.as_dvec3(), color)
    }

    /// An arrow from `from` to `to` with an automatic head size.
    pub fn arrow(&mut self, from: DVec3, to: DVec3, color: Color) -> ShapeHandle<'_> {
        self.arrow_with_head(from, to, 0.0, color)
    }

    /// An arrow with a head `head` meters long (0 = automatic: 20% of short arrows, about
    /// 10% of long ones).
    pub fn arrow_with_head(
        &mut self,
        from: DVec3,
        to: DVec3,
        head: f32,
        color: Color,
    ) -> ShapeHandle<'_> {
        let start = self.begin_shape();
        let d = to - from;
        let len = d.length();
        if len > 1e-9 {
            self.push(from, to, color);
            let head = if head > 0.0 {
                (head as f64).min(len)
            } else {
                (len * 0.2).min(0.5 + len * 0.1)
            };
            let (u, v, w) = basis(d);
            let back = to - w * head;
            let r = head * 0.35;
            for side in [u, -u, v, -v] {
                self.push(to, back + side * r, color);
            }
        }
        self.finish_shape(start)
    }

    /// Axis-aligned box between the corners `min` and `max`.
    pub fn aabb(&mut self, min: DVec3, max: DVec3, color: Color) -> ShapeHandle<'_> {
        let start = self.begin_shape();
        let center = (min + max) * 0.5;
        let half = (max - min) * 0.5;
        self.box_edges(|x, y, z| center + half * DVec3::new(x, y, z), color);
        self.finish_shape(start)
    }

    /// Oriented box: `center`, half extents along its local axes (meters), `rotation`.
    pub fn oriented_box(
        &mut self,
        center: DVec3,
        half_extents: Vec3,
        rotation: Quat,
        color: Color,
    ) -> ShapeHandle<'_> {
        let start = self.begin_shape();
        let m = dmat(rotation);
        let h = half_extents.as_dvec3();
        self.box_edges(|x, y, z| center + m * (h * DVec3::new(x, y, z)), color);
        self.finish_shape(start)
    }

    /// Sphere drawn as three great circles (around X, Y and Z).
    pub fn sphere(&mut self, center: DVec3, radius: f32, color: Color) -> ShapeHandle<'_> {
        let start = self.begin_shape();
        let r = radius as f64;
        self.ring(center, DVec3::X * r, DVec3::Y * r, color);
        self.ring(center, DVec3::Y * r, DVec3::Z * r, color);
        self.ring(center, DVec3::Z * r, DVec3::X * r, color);
        self.finish_shape(start)
    }

    /// Circle around `center` in the plane perpendicular to `normal`.
    pub fn circle(
        &mut self,
        center: DVec3,
        normal: Vec3,
        radius: f32,
        color: Color,
    ) -> ShapeHandle<'_> {
        let start = self.begin_shape();
        let (u, v, _) = basis(normal.as_dvec3());
        let r = radius as f64;
        self.ring(center, u * r, v * r, color);
        self.finish_shape(start)
    }

    /// Three axis-aligned lines through `at`, each `size` meters long.
    pub fn cross(&mut self, at: DVec3, size: f32, color: Color) -> ShapeHandle<'_> {
        let start = self.begin_shape();
        let h = size as f64 * 0.5;
        for axis in [DVec3::X, DVec3::Y, DVec3::Z] {
            self.push(at - axis * h, at + axis * h, color);
        }
        self.finish_shape(start)
    }

    /// A small cross ([`POINT_SIZE`] meters) marking a point.
    pub fn point(&mut self, at: DVec3, color: Color) -> ShapeHandle<'_> {
        self.cross(at, POINT_SIZE, color)
    }

    /// The local axes of `transform` (X red, Y green, Z blue), `size` meters long. Scale is
    /// ignored.
    pub fn axes(&mut self, transform: &Transform, size: f32) -> ShapeHandle<'_> {
        self.axes_at(transform.translation, transform.rotation, size)
    }

    /// The world axes of a [`GlobalTransform`] (X red, Y green, Z blue).
    pub fn axes_global(&mut self, transform: &GlobalTransform, size: f32) -> ShapeHandle<'_> {
        self.axes_at(transform.translation(), transform.rotation(), size)
    }

    /// Axes of a frame at `position` with `rotation` (X red, Y green, Z blue).
    pub fn axes_at(&mut self, position: DVec3, rotation: Quat, size: f32) -> ShapeHandle<'_> {
        let start = self.begin_shape();
        let m = dmat(rotation);
        let s = size as f64;
        for (axis, color) in [(m.x_axis, AXIS_X), (m.y_axis, AXIS_Y), (m.z_axis, AXIS_Z)] {
            self.push(position, position + axis * s, color);
        }
        self.finish_shape(start)
    }

    /// Capsule (a cylinder with hemispherical caps) around the segment `a`–`b`.
    pub fn capsule(&mut self, a: DVec3, b: DVec3, radius: f32, color: Color) -> ShapeHandle<'_> {
        if (b - a).length_squared() < 1e-18 {
            return self.sphere(a, radius, color);
        }
        let start = self.begin_shape();
        let (u, v, w) = basis(b - a);
        let r = radius as f64;
        let (u, v, w) = (u * r, v * r, w * r);
        self.ring(a, u, v, color);
        self.ring(b, u, v, color);
        for side in [u, -u, v, -v] {
            self.push(a + side, b + side, color);
        }
        let half = (self.segment_count() / 2).max(2);
        let pi = std::f64::consts::PI;
        // Caps: half circles bulging away from the segment, in two perpendicular planes.
        self.arc(b, (u, w), 0.0..pi, half, color);
        self.arc(b, (v, w), 0.0..pi, half, color);
        self.arc(a, (u, -w), 0.0..pi, half, color);
        self.arc(a, (v, -w), 0.0..pi, half, color);
        self.finish_shape(start)
    }

    /// Cylinder with flat caps around the segment `a`–`b`.
    pub fn cylinder(&mut self, a: DVec3, b: DVec3, radius: f32, color: Color) -> ShapeHandle<'_> {
        let start = self.begin_shape();
        let (u, v, _) = basis(b - a);
        let r = radius as f64;
        let (u, v) = (u * r, v * r);
        self.ring(a, u, v, color);
        self.ring(b, u, v, color);
        for side in [u, -u, v, -v] {
            self.push(a + side, b + side, color);
        }
        self.finish_shape(start)
    }

    /// Square grid of `cells` × `cells` cells of `cell` meters centered on `center`, in the
    /// plane perpendicular to `normal` (`Vec3::Y` for a ground grid). `cells` is capped at
    /// [`MAX_GRID_CELLS`].
    pub fn grid(
        &mut self,
        center: DVec3,
        normal: Vec3,
        cell: f32,
        cells: u32,
        color: Color,
    ) -> ShapeHandle<'_> {
        let start = self.begin_shape();
        let cells = cells.clamp(1, MAX_GRID_CELLS);
        let (u, v, _) = basis(normal.as_dvec3());
        let cell = cell as f64;
        let half = cells as f64 * cell * 0.5;
        for i in 0..=cells {
            let o = -half + i as f64 * cell;
            self.push(center + u * o - v * half, center + u * o + v * half, color);
            self.push(center + v * o - u * half, center + v * o + u * half, color);
        }
        self.finish_shape(start)
    }

    /// The frustum of a camera-relative view-projection matrix (wgpu clip space, as in
    /// `ViewData::view_proj`) for a camera at `origin`. Works for reverse-Z, standard-Z,
    /// perspective and orthographic matrices; edges longer than `max_distance` meters (an
    /// infinite far plane) are cut there.
    pub fn frustum(
        &mut self,
        view_proj: Mat4,
        origin: DVec3,
        max_distance: f32,
        color: Color,
    ) -> ShapeHandle<'_> {
        let start = self.begin_shape();
        if let Some(corners) = frustum_corners(view_proj, max_distance) {
            let p = |i: usize| origin + corners[i];
            // Corner index bits: 1 = +x, 2 = +y, 4 = far.
            for i in 0..8usize {
                for bit in [1usize, 2, 4] {
                    if i & bit == 0 {
                        self.push(p(i), p(i | bit), color);
                    }
                }
            }
        }
        self.finish_shape(start)
    }

    /// The frustum of `camera` at `transform` for an output with `aspect` = width / height.
    /// Perspective cameras have no far plane; their frustum is cut at `max_distance` meters.
    pub fn camera_frustum(
        &mut self,
        camera: &Camera,
        transform: &GlobalTransform,
        aspect: f32,
        max_distance: f32,
        color: Color,
    ) -> ShapeHandle<'_> {
        let proj = match camera.projection {
            Projection::Perspective { fov_y_deg, near } => {
                engine_render::frame::perspective_infinite_reverse(
                    fov_y_deg.clamp(1.0, 170.0).to_radians(),
                    aspect.max(1e-3),
                    near.max(1e-4),
                )
            }
            Projection::Orthographic { height, near, far } => {
                engine_render::frame::orthographic_reverse(height * aspect, height, near, far)
            }
        };
        let view = Mat4::from_quat(transform.rotation().inverse());
        self.frustum(proj * view, transform.translation(), max_distance, color)
    }

    /// Open polyline through `points`.
    pub fn polyline(
        &mut self,
        points: impl IntoIterator<Item = DVec3>,
        color: Color,
    ) -> ShapeHandle<'_> {
        self.path(points, false, color)
    }

    /// Closed polygon through `points` (the last point joins the first).
    pub fn polygon(
        &mut self,
        points: impl IntoIterator<Item = DVec3>,
        color: Color,
    ) -> ShapeHandle<'_> {
        self.path(points, true, color)
    }

    /// Polyline through `points`, optionally `closed`.
    pub fn path(
        &mut self,
        points: impl IntoIterator<Item = DVec3>,
        closed: bool,
        color: Color,
    ) -> ShapeHandle<'_> {
        let start = self.begin_shape();
        let mut iter = points.into_iter();
        if let Some(first) = iter.next() {
            let mut prev = first;
            let mut count = 1;
            for p in iter {
                self.push(prev, p, color);
                prev = p;
                count += 1;
            }
            if closed && count > 2 {
                self.push(prev, first, color);
            }
        }
        self.finish_shape(start)
    }

    /// Draw a [`DebugShape`] (the data form used by `debug.draw`).
    pub fn shape(&mut self, shape: &DebugShape, color: Color) -> ShapeHandle<'_> {
        match shape {
            DebugShape::Line { a, b } => self.line(*a, *b, color),
            DebugShape::Ray { origin, dir } => self.ray(*origin, *dir, color),
            DebugShape::Arrow { from, to, head } => self.arrow_with_head(*from, *to, *head, color),
            DebugShape::Aabb { min, max } => self.aabb(*min, *max, color),
            DebugShape::OrientedBox {
                center,
                half_extents,
                rotation,
            } => self.oriented_box(*center, *half_extents, *rotation, color),
            DebugShape::Sphere { center, radius } => self.sphere(*center, *radius, color),
            DebugShape::Circle {
                center,
                normal,
                radius,
            } => self.circle(*center, *normal, *radius, color),
            DebugShape::Cross { at, size } => self.cross(*at, *size, color),
            DebugShape::Axes {
                position,
                rotation,
                size,
            } => self.axes_at(*position, *rotation, *size),
            DebugShape::Capsule { a, b, radius } => self.capsule(*a, *b, *radius, color),
            DebugShape::Cylinder { a, b, radius } => self.cylinder(*a, *b, *radius, color),
            DebugShape::Grid {
                center,
                normal,
                cell,
                cells,
            } => self.grid(*center, *normal, *cell, *cells, color),
            DebugShape::Frustum {
                view_proj,
                origin,
                max_distance,
            } => self.frustum(*view_proj, *origin, *max_distance, color),
            DebugShape::Path { points, closed } => {
                self.path(points.iter().copied(), *closed, color)
            }
        }
    }

    /// Draw a [`DebugShape`] with a color, duration and depth mode.
    pub fn styled(&mut self, shape: &DebugShape, style: ShapeStyle) -> ShapeHandle<'_> {
        self.shape(shape, style.color)
            .depth(style.depth)
            .duration(style.duration)
    }
}

/// Camera-relative corners of a view-projection frustum (index bits: 1 = +x, 2 = +y,
/// 4 = far end). `None` for a degenerate matrix.
fn frustum_corners(view_proj: Mat4, max_distance: f32) -> Option<[DVec3; 8]> {
    let inv = view_proj.as_dmat4().inverse();
    if !inv.is_finite() {
        return None;
    }
    let max = max_distance.max(1e-3) as f64;
    let mut out = [DVec3::ZERO; 8];
    for i in 0..4usize {
        let x = if i & 1 != 0 { 1.0 } else { -1.0 };
        let y = if i & 2 != 0 { 1.0 } else { -1.0 };
        // Points at both ends of clip-space depth (0 and 1). Either can be the near end
        // (reverse vs standard Z) and either can be at infinity (w = 0).
        let h0 = inv * Vec4::new(x, y, 0.0, 1.0).as_dvec4();
        let h1 = inv * Vec4::new(x, y, 1.0, 1.0).as_dvec4();
        let finite = |h: DVec4| h.w.abs() > 1e-9 * h.truncate().length().max(1e-30);
        let (near, far) = match (finite(h0), finite(h1)) {
            (true, true) => {
                let (p0, p1) = (h0.truncate() / h0.w, h1.truncate() / h1.w);
                // The camera sits at the relative origin: the end closer to it is "near". On a
                // tie (an orthographic near plane behind the camera, as far behind as the far
                // plane is in front) use clip z = 1, the near plane of the engine's reverse-Z.
                let (d0, d1) = (p0.length_squared(), p1.length_squared());
                let (near, far) = if d0 < d1 * (1.0 - 1e-6) {
                    (p0, p1)
                } else {
                    (p1, p0)
                };
                let edge = far - near;
                if edge.length() > max {
                    (near, near + edge.normalize() * max)
                } else {
                    (near, far)
                }
            }
            (true, false) | (false, true) => {
                let (finite_h, inf_h) = if finite(h0) { (h0, h1) } else { (h1, h0) };
                let near = finite_h.truncate() / finite_h.w;
                let mut dir = inf_h.truncate().try_normalize()?;
                if dir.dot(near) < 0.0 {
                    dir = -dir;
                }
                (near, near + dir * max)
            }
            (false, false) => return None,
        };
        out[i] = near;
        out[i | 4] = far;
    }
    out.iter().all(|p| p.is_finite()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::*;
    use crate::draw::DebugLine;

    fn shape_lines(f: impl FnOnce(&mut DebugDraw)) -> Vec<DebugLine> {
        let mut d = DebugDraw::default();
        d.begin_frame(0.0);
        f(&mut d);
        d.lines(DepthMode::Tested)
            .chain(d.lines(DepthMode::OnTop))
            .copied()
            .collect()
    }

    fn endpoints(lines: &[DebugLine]) -> impl Iterator<Item = DVec3> + '_ {
        lines.iter().flat_map(|l| [l.a, l.b])
    }

    #[test]
    fn sphere_is_three_circles_on_the_surface() {
        let c = DVec3::new(1.0e8, -3.0, 2.0);
        let lines = shape_lines(|d| {
            d.sphere(c, 2.0, RED);
        });
        assert_eq!(lines.len(), 3 * 32);
        for p in endpoints(&lines) {
            assert!(((p - c).length() - 2.0).abs() < 1e-6, "{p:?}");
        }
        assert!(lines.iter().all(|l| l.color == RED));
    }

    #[test]
    fn circle_lies_in_its_plane() {
        let n = Vec3::new(1.0, 2.0, -0.5).normalize();
        let c = DVec3::new(5.0, 6.0, 7.0);
        let lines = shape_lines(|d| {
            d.circle(c, n, 3.0, GREEN);
        });
        assert_eq!(lines.len(), 32);
        for p in endpoints(&lines) {
            let rel = p - c;
            assert!(rel.dot(n.as_dvec3()).abs() < 1e-5);
            assert!((rel.length() - 3.0).abs() < 1e-5);
        }
        // Closed: the last segment ends where the first starts.
        assert!(lines[31].b.distance(lines[0].a) < 1e-9);
    }

    #[test]
    fn boxes_have_twelve_edges_at_their_corners() {
        let lines = shape_lines(|d| {
            d.aabb(DVec3::new(-1.0, 0.0, -2.0), DVec3::new(1.0, 3.0, 2.0), BLUE);
        });
        assert_eq!(lines.len(), 12);
        for p in endpoints(&lines) {
            assert!(
                p.x.abs() == 1.0 && (p.y == 0.0 || p.y == 3.0) && p.z.abs() == 2.0,
                "{p:?}"
            );
        }
        for l in &lines {
            let len = l.a.distance(l.b);
            assert!([2.0, 3.0, 4.0].contains(&len), "edge {len}");
        }

        let rot = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let lines = shape_lines(|d| {
            d.oriented_box(DVec3::ZERO, Vec3::new(2.0, 0.5, 0.25), rot, BLUE);
        });
        assert_eq!(lines.len(), 12);
        // Local X (half 2) now points along world -Z.
        for p in endpoints(&lines) {
            assert!(
                (p.z.abs() - 2.0).abs() < 1e-6 && (p.x.abs() - 0.25).abs() < 1e-6,
                "{p:?}"
            );
        }
    }

    #[test]
    fn arrow_has_shaft_and_head() {
        let lines = shape_lines(|d| {
            d.arrow(DVec3::ZERO, DVec3::new(0.0, 0.0, -10.0), YELLOW);
        });
        assert_eq!(lines.len(), 5);
        let tip = DVec3::new(0.0, 0.0, -10.0);
        assert_eq!(lines[0].b, tip);
        for l in &lines[1..] {
            assert_eq!(l.a, tip);
            assert!(
                l.b.z > -10.0 && l.b.z < -8.0,
                "head goes back from the tip: {:?}",
                l.b
            );
        }
        // Zero-length arrows draw nothing.
        assert!(
            shape_lines(|d| {
                d.arrow(DVec3::ONE, DVec3::ONE, RED);
            })
            .is_empty()
        );
    }

    #[test]
    fn ray_cross_axes_and_point() {
        let lines = shape_lines(|d| {
            d.ray(DVec3::X, Vec3::new(0.0, 2.0, 0.0), RED);
        });
        assert_eq!(lines[0].b, DVec3::new(1.0, 2.0, 0.0));

        let lines = shape_lines(|d| {
            d.cross(DVec3::ZERO, 2.0, RED);
            d.point(DVec3::ZERO, RED);
        });
        assert_eq!(lines.len(), 6);
        assert_eq!(lines[0].a.distance(lines[0].b), 2.0);

        let t = Transform::from_xyz(0.0, 1.0, 0.0)
            .with_rotation(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2));
        let lines = shape_lines(|d| {
            d.axes(&t, 2.0);
        });
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].color, AXIS_X);
        // X rotated 90° about Z points along +Y.
        assert!(
            lines[0].b.abs_diff_eq(DVec3::new(0.0, 3.0, 0.0), 1e-6),
            "{:?}",
            lines[0].b
        );
    }

    #[test]
    fn capsule_and_cylinder_wrap_the_segment() {
        let a = DVec3::new(0.0, 0.0, 0.0);
        let b = DVec3::new(0.0, 2.0, 0.0);
        let lines = shape_lines(|d| {
            d.capsule(a, b, 0.5, CYAN);
        });
        assert_eq!(lines.len(), 2 * 32 + 4 + 4 * 16);
        let max_y = endpoints(&lines).map(|p| p.y).fold(f64::MIN, f64::max);
        let min_y = endpoints(&lines).map(|p| p.y).fold(f64::MAX, f64::min);
        assert!(
            (max_y - 2.5).abs() < 1e-6 && (min_y + 0.5).abs() < 1e-6,
            "{min_y}..{max_y}"
        );
        for p in endpoints(&lines) {
            // Every point is `radius` from the segment.
            let t = p.y.clamp(0.0, 2.0);
            assert!((p.distance(DVec3::new(0.0, t, 0.0)) - 0.5).abs() < 1e-6);
        }

        let lines = shape_lines(|d| {
            d.cylinder(a, b, 0.5, CYAN);
        });
        assert_eq!(lines.len(), 2 * 32 + 4);
        let max_y = endpoints(&lines).map(|p| p.y).fold(f64::MIN, f64::max);
        assert!((max_y - 2.0).abs() < 1e-9, "{max_y}");
    }

    #[test]
    fn grid_and_paths() {
        let lines = shape_lines(|d| {
            d.grid(DVec3::new(10.0, 0.0, 0.0), Vec3::Y, 1.0, 4, GRAY);
        });
        assert_eq!(lines.len(), 10);
        for p in endpoints(&lines) {
            assert!(p.y.abs() < 1e-12);
            assert!((p.x - 10.0).abs() <= 2.0 && p.z.abs() <= 2.0);
        }
        let square = [DVec3::ZERO, DVec3::X, DVec3::new(1.0, 0.0, 1.0), DVec3::Z];
        assert_eq!(
            shape_lines(|d| {
                d.polyline(square, RED);
            })
            .len(),
            3
        );
        assert_eq!(
            shape_lines(|d| {
                d.polygon(square, RED);
            })
            .len(),
            4
        );
        assert!(
            shape_lines(|d| {
                d.polygon([DVec3::ZERO], RED);
            })
            .is_empty()
        );
    }

    #[test]
    fn frustum_of_reverse_z_infinite_camera() {
        let proj = engine_render::frame::perspective_infinite_reverse(1.0, 16.0 / 9.0, 0.1);
        let rot = Quat::from_rotation_y(0.3);
        let view = Mat4::from_quat(rot.inverse());
        let origin = DVec3::new(1.0e8, 5.0, 0.0);
        let lines = shape_lines(|d| {
            d.frustum(proj * view, origin, 50.0, MAGENTA);
        });
        assert_eq!(lines.len(), 12);
        let forward = (rot * Vec3::NEG_Z).as_dvec3();
        let depths: Vec<f64> = endpoints(&lines)
            .map(|p| (p - origin).dot(forward))
            .collect();
        // Near corners sit on the near plane; far corners are cut ~50 m out, all in front.
        assert!(depths.iter().all(|d| *d > 0.0999), "{depths:?}");
        let near = depths.iter().filter(|d| (**d - 0.1).abs() < 1e-4).count();
        assert_eq!(near, 12, "each near corner touches 3 edges: {depths:?}");
        // Far corners continue each corner ray 50 m past the near plane.
        let far: Vec<f64> = endpoints(&lines)
            .map(|p| (p - origin).length())
            .filter(|d| *d > 1.0)
            .collect();
        assert_eq!(far.len(), 12);
        assert!(far.iter().all(|d| (50.0..50.3).contains(d)), "{far:?}");

        // The camera helper produces the same frustum.
        let cam = Camera {
            projection: Projection::Perspective {
                fov_y_deg: 1.0f32.to_degrees(),
                near: 0.1,
            },
            ..Default::default()
        };
        let gt = GlobalTransform::from(Transform::from_translation(origin).with_rotation(rot));
        let cam_lines = shape_lines(|d| {
            d.camera_frustum(&cam, &gt, 16.0 / 9.0, 50.0, MAGENTA);
        });
        assert_eq!(cam_lines.len(), 12);
        for (a, b) in lines.iter().zip(&cam_lines) {
            assert!(a.a.distance(b.a) < 0.01 && a.b.distance(b.b) < 0.01);
        }
    }

    #[test]
    fn frustum_of_orthographic_box() {
        let proj = engine_render::frame::orthographic_reverse(4.0, 2.0, 0.0, 10.0);
        let lines = shape_lines(|d| {
            d.frustum(proj, DVec3::ZERO, 100.0, MAGENTA);
        });
        assert_eq!(lines.len(), 12);
        for p in endpoints(&lines) {
            assert!(
                (p.x.abs() - 2.0).abs() < 1e-5 && (p.y.abs() - 1.0).abs() < 1e-5,
                "{p:?}"
            );
            assert!(p.z.abs() < 1e-4 || (p.z + 10.0).abs() < 1e-4, "{p:?}");
        }
        // Near plane behind the camera (the default orthographic camera): both ends are
        // equally far from the camera, so the reverse-Z near plane (clip z = 1) is used as the
        // near end for every camera rotation, not whichever end rounding favors.
        let proj = engine_render::frame::orthographic_reverse(4.0, 2.0, -10.0, 10.0);
        for i in 0..64 {
            let rot = Quat::from_euler(
                engine_core::math::EulerRot::YXZ,
                i as f32 * 0.37,
                i as f32 * 0.11 - 3.0,
                i as f32 * 0.05,
            );
            let view = Mat4::from_quat(rot.inverse());
            let corners = frustum_corners(proj * view, 5.0).unwrap();
            let back = (rot * Vec3::Z).as_dvec3();
            for c in &corners[..4] {
                assert!((c.dot(back) - 10.0).abs() < 1e-3, "near corner {c:?} ({i})");
            }
            for c in &corners[4..] {
                assert!((c.dot(back) - 5.0).abs() < 1e-3, "cut corner {c:?} ({i})");
            }
        }

        // A singular matrix draws nothing.
        assert!(
            shape_lines(|d| {
                d.frustum(Mat4::ZERO, DVec3::ZERO, 10.0, RED);
            })
            .is_empty()
        );
    }

    #[test]
    fn shapes_round_trip_through_json_and_draw() {
        let shapes = vec![
            DebugShape::Line {
                a: DVec3::ZERO,
                b: DVec3::X,
            },
            DebugShape::OrientedBox {
                center: DVec3::Y,
                half_extents: Vec3::ONE,
                rotation: Quat::IDENTITY,
            },
            DebugShape::Path {
                points: vec![DVec3::ZERO, DVec3::X, DVec3::Y],
                closed: true,
            },
        ];
        for s in &shapes {
            let v = serde_json::to_value(s).unwrap();
            let back: DebugShape = serde_json::from_value(v.clone()).unwrap();
            assert_eq!(&back, s, "{v}");
        }
        let v = serde_json::to_value(&shapes[1]).unwrap();
        assert!(v.get("box").is_some(), "{v}");
        // Defaults and aliases.
        let p: DebugShape =
            serde_json::from_value(serde_json::json!({"point": {"at": [1, 2, 3]}})).unwrap();
        assert_eq!(
            p,
            DebugShape::Cross {
                at: DVec3::new(1.0, 2.0, 3.0),
                size: POINT_SIZE
            }
        );
        let g: DebugShape =
            serde_json::from_value(serde_json::json!({"grid": {"center": [0, 0, 0]}})).unwrap();
        let lines = shape_lines(|d| {
            d.styled(
                &g,
                ShapeStyle {
                    color: WHITE,
                    duration: 1.0,
                    depth: DepthMode::OnTop,
                },
            );
        });
        assert_eq!(lines.len(), 22);
    }
}
