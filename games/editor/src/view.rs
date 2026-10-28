//! The scene view's camera math: rays through the cursor, projecting world points to the
//! screen, and the matrices the transform gizmo needs. Everything is camera-relative, so it
//! stays precise far from the world origin.

use engine::prelude::*;

/// How far in front of and behind the camera an orthographic view sees, meters: the camera keeps
/// its perspective position, and what is behind it still shows.
pub const ORTHO_DEPTH: f32 = 5000.0;

/// What the editor camera sees this frame, in egui points.
#[derive(Debug, Clone, Copy)]
pub struct ViewInfo {
    /// Camera position (world meters).
    pub origin: DVec3,
    pub rotation: Quat,
    /// Vertical field of view, radians.
    pub fov_y: f32,
    pub near: f32,
    /// Orthographic: meters seen top to bottom (`fov_y` is unused then).
    pub ortho: Option<f32>,
    /// The rectangle the scene is drawn into (points).
    pub screen: egui::Rect,
}

impl ViewInfo {
    pub fn aspect(&self) -> f32 {
        self.screen.width().max(1.0) / self.screen.height().max(1.0)
    }

    /// World-space ray (origin, unit direction) through a screen point.
    pub fn ray(&self, pos: egui::Pos2) -> (DVec3, Vec3) {
        let x = (pos.x - self.screen.min.x) / self.screen.width() * 2.0 - 1.0;
        let y = 1.0 - (pos.y - self.screen.min.y) / self.screen.height() * 2.0;
        if let Some(h) = self.ortho {
            // Parallel rays from the camera plane.
            let half = Vec3::new(x * h * 0.5 * self.aspect(), y * h * 0.5, 0.0);
            return (
                self.origin + (self.rotation * half).as_dvec3(),
                self.rotation * Vec3::NEG_Z,
            );
        }
        let t = (self.fov_y * 0.5).tan();
        let dir = Vec3::new(x * t * self.aspect(), y * t, -1.0).normalize();
        (self.origin, self.rotation * dir)
    }

    /// Screen position of a world point, or `None` behind the camera.
    pub fn project(&self, world: DVec3) -> Option<egui::Pos2> {
        let rel = self.rotation.inverse() * (world - self.origin).as_vec3();
        let (x, y) = match self.ortho {
            Some(h) => {
                if rel.z.abs() > ORTHO_DEPTH {
                    return None;
                }
                (rel.x / (h * 0.5 * self.aspect()), rel.y / (h * 0.5))
            }
            None => {
                if rel.z > -self.near * 0.5 {
                    return None;
                }
                let t = (self.fov_y * 0.5).tan();
                (rel.x / (-rel.z * t * self.aspect()), rel.y / (-rel.z * t))
            }
        };
        Some(egui::pos2(
            self.screen.min.x + (x + 1.0) * 0.5 * self.screen.width(),
            self.screen.min.y + (1.0 - y) * 0.5 * self.screen.height(),
        ))
    }

    /// Meters per point at distance `d` from the camera (for screen-constant sizes).
    pub fn meters_per_point(&self, d: f64) -> f64 {
        if let Some(h) = self.ortho {
            return h as f64 / self.screen.height().max(1.0) as f64;
        }
        2.0 * d * (self.fov_y as f64 * 0.5).tan() / self.screen.height().max(1.0) as f64
    }

    /// View matrix for the gizmo: the camera at the origin (positions are passed relative).
    pub fn gizmo_view(&self) -> glam::DMat4 {
        glam::DMat4::from_quat(self.rotation.as_dquat()).inverse()
    }

    /// OpenGL-style projection for the gizmo (it unprojects with z in -1..1).
    pub fn gizmo_proj(&self) -> glam::DMat4 {
        if let Some(h) = self.ortho {
            let (w, h) = ((h * self.aspect()) as f64 * 0.5, h as f64 * 0.5);
            let depth = ORTHO_DEPTH as f64;
            return glam::dcamera::rh::proj::opengl::orthographic(-w, w, -h, h, -depth, depth);
        }
        glam::dcamera::rh::proj::opengl::perspective(
            self.fov_y as f64,
            self.aspect() as f64,
            self.near.max(0.01) as f64,
            1.0e6,
        )
    }
}

/// Row-major mint matrix from a glam one (transform-gizmo takes mint).
pub fn mint_rows(m: glam::DMat4) -> transform_gizmo_egui::mint::RowMatrix4<f64> {
    let c = m.transpose().to_cols_array_2d();
    transform_gizmo_egui::mint::RowMatrix4 {
        x: c[0].into(),
        y: c[1].into(),
        z: c[2].into(),
        w: c[3].into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> ViewInfo {
        ViewInfo {
            origin: DVec3::new(1.0e6, 2.0, 5.0),
            rotation: Quat::from_rotation_y(0.4) * Quat::from_rotation_x(-0.2),
            fov_y: 65f32.to_radians(),
            near: 0.05,
            ortho: None,
            screen: egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1600.0, 900.0)),
        }
    }

    #[test]
    fn ray_and_project_agree() {
        let v = view();
        for p in [
            egui::pos2(800.0, 450.0),
            egui::pos2(100.0, 50.0),
            egui::pos2(1500.0, 800.0),
        ] {
            let (o, d) = v.ray(p);
            let back = v.project(o + (d * 37.0).as_dvec3()).unwrap();
            assert!((back - p).length() < 0.01, "{p:?} -> {back:?}");
        }
    }

    #[test]
    fn orthographic_rays_are_parallel_and_project_back() {
        let v = ViewInfo {
            ortho: Some(12.0),
            ..view()
        };
        let (o1, d1) = v.ray(egui::pos2(100.0, 50.0));
        let (o2, d2) = v.ray(egui::pos2(1500.0, 800.0));
        assert!((d1 - d2).length() < 1e-6);
        assert!((o1 - o2).length() > 1.0);
        for (o, d, p) in [
            (o1, d1, egui::pos2(100.0, 50.0)),
            (o2, d2, egui::pos2(1500.0, 800.0)),
        ] {
            let back = v.project(o + (d * 37.0).as_dvec3()).unwrap();
            assert!((back - p).length() < 0.01, "{p:?} -> {back:?}");
        }
        // 12 m over 900 points.
        assert!((v.meters_per_point(100.0) - 12.0 / 900.0).abs() < 1e-9);
    }

    #[test]
    fn gizmo_matrices_project_like_the_view() {
        for v in [
            view(),
            ViewInfo {
                ortho: Some(8.0),
                ..view()
            },
        ] {
            gizmo_matches(v);
        }
    }

    fn gizmo_matches(v: ViewInfo) {
        let world = v.origin + DVec3::new(3.0, -1.0, -20.0);
        let expected = v.project(world).unwrap();
        let clip = v.gizmo_proj() * v.gizmo_view() * (world - v.origin).extend(1.0);
        let ndc = clip.truncate() / clip.w;
        let x = (ndc.x + 1.0) * 0.5 * 1600.0;
        let y = (1.0 - ndc.y) * 0.5 * 900.0;
        assert!((x as f32 - expected.x).abs() < 0.01);
        assert!((y as f32 - expected.y).abs() < 0.01);
    }
}
