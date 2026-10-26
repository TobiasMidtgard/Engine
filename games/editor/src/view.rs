//! The scene view's camera math: rays through the cursor, projecting world points to the
//! screen, and the matrices the transform gizmo needs. Everything is camera-relative, so it
//! stays precise far from the world origin.

use engine::prelude::*;

/// What the editor camera sees this frame, in egui points.
#[derive(Debug, Clone, Copy)]
pub struct ViewInfo {
    /// Camera position (world meters).
    pub origin: DVec3,
    pub rotation: Quat,
    /// Vertical field of view, radians.
    pub fov_y: f32,
    pub near: f32,
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
        let t = (self.fov_y * 0.5).tan();
        let dir = Vec3::new(x * t * self.aspect(), y * t, -1.0).normalize();
        (self.origin, self.rotation * dir)
    }

    /// Screen position of a world point, or `None` behind the camera.
    pub fn project(&self, world: DVec3) -> Option<egui::Pos2> {
        let rel = self.rotation.inverse() * (world - self.origin).as_vec3();
        if rel.z > -self.near * 0.5 {
            return None;
        }
        let t = (self.fov_y * 0.5).tan();
        let x = rel.x / (-rel.z * t * self.aspect());
        let y = rel.y / (-rel.z * t);
        Some(egui::pos2(
            self.screen.min.x + (x + 1.0) * 0.5 * self.screen.width(),
            self.screen.min.y + (1.0 - y) * 0.5 * self.screen.height(),
        ))
    }

    /// Meters per point at distance `d` from the camera (for screen-constant sizes).
    pub fn meters_per_point(&self, d: f64) -> f64 {
        2.0 * d * (self.fov_y as f64 * 0.5).tan() / self.screen.height().max(1.0) as f64
    }

    /// View matrix for the gizmo: the camera at the origin (positions are passed relative).
    pub fn gizmo_view(&self) -> glam::DMat4 {
        glam::DMat4::from_quat(self.rotation.as_dquat()).inverse()
    }

    /// OpenGL-style projection for the gizmo (it unprojects with z in -1..1).
    pub fn gizmo_proj(&self) -> glam::DMat4 {
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
    fn gizmo_matrices_project_like_the_view() {
        let v = view();
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
