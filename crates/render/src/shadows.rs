//! Cascaded shadow maps for the sun.
//!
//! Each cascade covers a slice of the view frustum with a bounding sphere whose radius only
//! depends on the slice (not the camera rotation), and its center is snapped to whole shadow
//! texels in world space (f64), so shadows don't shimmer when the camera moves or turns.
//! Casters between the sun and a cascade are "pancaked" onto its near plane in the shader.

use bytemuck::{Pod, Zeroable};
use engine_core::prelude::*;

use crate::settings::RenderSettings;

pub const MAX_CASCADES: usize = 4;

/// Per-frame cascade data (camera-relative).
#[derive(Debug, Clone, Copy)]
pub struct Cascades {
    pub count: u32,
    pub map_size: u32,
    /// Camera-relative → light clip space.
    pub view_proj: [Mat4; MAX_CASCADES],
    /// Light view (camera-relative → light space, looking down −Z).
    pub light_view: [Mat4; MAX_CASCADES],
    /// Half extent of each cascade's square, meters.
    pub half_extent: [f32; MAX_CASCADES],
    /// Depth range of each cascade, meters.
    pub depth_range: [f32; MAX_CASCADES],
    /// Far view distance of each cascade.
    pub splits: [f32; 4],
}

impl Default for Cascades {
    fn default() -> Self {
        Self {
            count: 0,
            map_size: 2048,
            view_proj: [Mat4::IDENTITY; MAX_CASCADES],
            light_view: [Mat4::IDENTITY; MAX_CASCADES],
            half_extent: [0.0; MAX_CASCADES],
            depth_range: [0.0; MAX_CASCADES],
            splits: [0.0; 4],
        }
    }
}

impl Cascades {
    /// Whether a camera-relative bounding sphere can cast a shadow into cascade `i`.
    /// Spheres toward the sun are kept (they cast onto the cascade from outside it).
    pub fn sphere_in_cascade(&self, i: usize, center: Vec3, radius: f32) -> bool {
        let p = self.light_view[i].transform_point3(center);
        let e = self.half_extent[i] + radius;
        p.x.abs() <= e && p.y.abs() <= e && -p.z - radius <= self.depth_range[i]
    }
}

/// Top-down depth map around the camera that records the highest surface over each spot.
///
/// Shading uses it for sky visibility (caves, rooms and overhangs don't get sky light), and
/// weather effects use it to keep rain and snow off covered ground (`sky_visibility` in
/// `lib_frame.wgsl`). Features draw into it from `draw_shadow` with
/// [`SKY_OCCLUSION_VIEW`] as the cascade index, using the same shadow pipeline as for the sun.
#[derive(Debug, Clone, Copy)]
pub struct SkyOcclusionView {
    pub enabled: bool,
    pub map_size: u32,
    /// Width of the covered square, meters.
    pub extent: f32,
    /// Depth covered: from `depth_range / 2` above the camera to as far below it.
    pub depth_range: f32,
    /// Camera-relative → occlusion clip space (orthographic, looking down, standard Z).
    pub view_proj: Mat4,
    /// Camera-relative → occlusion view space (looking down −Z), for culling.
    pub view: Mat4,
}

/// Cascade index passed to `RenderFeature::draw_shadow` for the sky occlusion map.
pub const SKY_OCCLUSION_VIEW: usize = MAX_CASCADES;

impl Default for SkyOcclusionView {
    fn default() -> Self {
        Self {
            enabled: false,
            map_size: 512,
            extent: 256.0,
            depth_range: 800.0,
            view_proj: Mat4::IDENTITY,
            view: Mat4::IDENTITY,
        }
    }
}

impl SkyOcclusionView {
    /// Build the view for a camera at world position `origin`. The square is snapped to whole
    /// texels in world space so the map doesn't shimmer as the camera moves.
    pub fn new(origin: DVec3, map_size: u32, extent: f32, depth_range: f32, enabled: bool) -> Self {
        let texel = extent as f64 / map_size.max(1) as f64;
        let snapped = DVec3::new(
            (origin.x / texel).floor() * texel,
            origin.y,
            (origin.z / texel).floor() * texel,
        );
        let eye = (snapped - origin).as_vec3() + Vec3::Y * (depth_range * 0.5);
        // Looking straight down (−Y) with −Z as "up" in the map.
        let rotation = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
        let view = Mat4::from_rotation_translation(rotation, eye).inverse();
        let proj = crate::frame::orthographic_standard(extent * 0.5, 0.0, depth_range);
        Self {
            enabled,
            map_size,
            extent,
            depth_range,
            view_proj: proj * view,
            view,
        }
    }

    /// Whether a camera-relative bounding sphere touches the covered box.
    pub fn contains_sphere(&self, center: Vec3, radius: f32) -> bool {
        let p = self.view.transform_point3(center);
        let e = self.extent * 0.5 + radius;
        p.x.abs() <= e
            && p.y.abs() <= e
            && -p.z + radius >= 0.0
            && -p.z - radius <= self.depth_range
    }

    /// `sky_occlusion_params` in the frame uniform: enabled, extent, texel size, depth range.
    pub fn params(&self) -> [f32; 4] {
        [
            if self.enabled { 1.0 } else { 0.0 },
            self.extent,
            self.extent / self.map_size.max(1) as f32,
            self.depth_range,
        ]
    }
}

/// Layer of the shadow map array that holds the far shadow map (after the cascades).
pub const FAR_SHADOW_LAYER: usize = MAX_CASCADES;

/// One coarse sun shadow map around the camera for what lies beyond the cascades
/// (`RenderSettings::shadow_distance`), out to `RenderSettings::far_shadow_distance`: mountains
/// shade the valley at low sun and ridges shade themselves kilometers away.
///
/// Only big casters draw into it (terrain, mountains, cliffs, large buildings; see
/// [`FarShadowView::casts`]), from [`RenderFeature::draw_far_shadow`](crate::RenderFeature::draw_far_shadow).
/// It covers a square around the camera whose size doesn't depend on where the camera looks,
/// snapped to whole texels in world space, so it neither shimmers nor swims as the camera turns
/// and moves. Shading blends into it over the last tenth of the cascades (`shadow_factor` in
/// `lib_frame.wgsl`).
#[derive(Debug, Clone, Copy)]
pub struct FarShadowView {
    pub enabled: bool,
    pub map_size: u32,
    /// Half the width of the covered square (and the reach from the camera), meters.
    pub half_extent: f32,
    /// Depth range from the light's eye, meters.
    pub depth_range: f32,
    /// Camera-relative → far shadow clip space (orthographic, standard Z).
    pub view_proj: Mat4,
    /// Camera-relative → light view space (looking down −Z), for culling.
    pub light_view: Mat4,
    /// Casters smaller than this bounding radius (meters, two texels) are left out.
    pub min_caster_radius: f32,
}

impl Default for FarShadowView {
    fn default() -> Self {
        Self {
            enabled: false,
            map_size: 2048,
            half_extent: 0.0,
            depth_range: 0.0,
            view_proj: Mat4::IDENTITY,
            light_view: Mat4::IDENTITY,
            min_caster_radius: f32::INFINITY,
        }
    }
}

impl FarShadowView {
    /// The far view for a camera at world position `origin` with the sun toward `sun_dir`,
    /// reaching `distance` meters, at `map_size`² texels. Disabled when `distance` doesn't reach
    /// past `cascade_reach` (the cascades already cover it).
    pub fn new(
        origin: DVec3,
        sun_dir: Vec3,
        map_size: u32,
        distance: f32,
        cascade_reach: f32,
    ) -> Self {
        if !(distance > cascade_reach && distance > 0.0) || sun_dir.length_squared() < 0.5 {
            return Self::default();
        }
        let map_size = map_size.max(256);
        let radius = distance as f64;
        let texel = 2.0 * radius / map_size as f64;
        let light_rot = look_rotation_f64(-sun_dir.as_dvec3());
        let mut ls = light_rot.inverse() * origin;
        ls.x = (ls.x / texel).floor() * texel;
        ls.y = (ls.y / texel).floor() * texel;
        let center_rel = (light_rot * ls - origin).as_vec3();
        // Casters up to two radii toward the sun (mountains across the valley) get their real
        // depth; anything further is pancaked onto the near plane.
        let back = 2.0 * distance + 2000.0;
        let eye = center_rel + sun_dir * back;
        let light_view =
            Mat4::from_quat(light_rot.as_quat().inverse()) * Mat4::from_translation(-eye);
        let depth_range = back + distance;
        let proj = crate::frame::orthographic_standard(distance, 0.0, depth_range);
        Self {
            enabled: true,
            map_size,
            half_extent: distance,
            depth_range,
            view_proj: proj * light_view,
            light_view,
            min_caster_radius: 2.0 * texel as f32,
        }
    }

    /// Size of one texel, meters.
    pub fn texel(&self) -> f32 {
        2.0 * self.half_extent / self.map_size.max(1) as f32
    }

    /// Whether a camera-relative bounding sphere touches the covered box (or lies toward the
    /// sun from it, so it can shade it).
    pub fn contains_sphere(&self, center: Vec3, radius: f32) -> bool {
        if !self.enabled {
            return false;
        }
        let p = self.light_view.transform_point3(center);
        let e = self.half_extent + radius;
        p.x.abs() <= e && p.y.abs() <= e && -p.z - radius <= self.depth_range
    }

    /// Whether a caster with this camera-relative bounding sphere belongs in the far map: big
    /// enough to show at its resolution, and inside it.
    pub fn casts(&self, center: Vec3, radius: f32) -> bool {
        radius >= self.min_caster_radius && self.contains_sphere(center, radius)
    }

    /// `far_shadow_params` in the frame uniform: reach (m, 0 = off), texel size (m), clip depth
    /// per meter, shadow map layer.
    pub fn params(&self) -> [f32; 4] {
        if !self.enabled {
            return [0.0, 1.0, 0.0, FAR_SHADOW_LAYER as f32];
        }
        [
            self.half_extent,
            self.texel(),
            1.0 / self.depth_range.max(1e-3),
            FAR_SHADOW_LAYER as f32,
        ]
    }
}

/// GPU layout of `struct ShadowView` in `mesh_shadow.wgsl` (and any feature's shadow shader).
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct ShadowViewUniform {
    pub view_proj: [[f32; 4]; 4],
    pub wind: [f32; 4],
    pub time: [f32; 4],
    pub camera_frac: [f32; 4],
}

/// Cascade split distances: blend of logarithmic and uniform (`lambda` 1 = log).
pub fn split_distances(near: f32, far: f32, count: usize, lambda: f32) -> [f32; 4] {
    let mut out = [far; 4];
    for (i, o) in out.iter_mut().enumerate().take(count) {
        let f = (i + 1) as f32 / count as f32;
        let log = near * (far / near).powf(f);
        let uni = near + (far - near) * f;
        *o = lambda * log + (1.0 - lambda) * uni;
    }
    out
}

#[allow(clippy::too_many_arguments)]
pub fn compute_cascades(
    settings: &RenderSettings,
    origin: DVec3,
    rotation: Quat,
    near: f32,
    fov_y: f32,
    aspect: f32,
    orthographic: bool,
    sun_dir: Vec3,
    proj: &Mat4,
) -> Cascades {
    let count = MAX_CASCADES;
    let map_size = settings.shadow_map_size.max(256);
    let far = settings.shadow_distance.max(near * 4.0 + 1.0);
    // The first cascade starts a little in front of the camera to give it more resolution.
    let first_near = near.max(0.1);
    let splits = split_distances(
        first_near.max(1.0),
        far,
        count,
        settings.shadow_split_lambda,
    );

    let forward = (rotation * Vec3::NEG_Z).as_dvec3();
    let (tx, ty) = if orthographic {
        (1.0 / proj.x_axis.x as f64, 1.0 / proj.y_axis.y as f64)
    } else {
        let ty = (fov_y as f64 * 0.5).tan();
        (ty * aspect as f64, ty)
    };
    let s = tx * tx + ty * ty;

    // Light basis: looking along −sun_dir.
    let light_rot = look_rotation_f64(-sun_dir.as_dvec3());
    let light_rot_inv = light_rot.inverse();

    let mut out = Cascades {
        count: count as u32,
        map_size,
        splits,
        ..Default::default()
    };
    let mut slice_near = first_near as f64;
    for (i, &split) in splits.iter().enumerate().take(count) {
        let slice_far = split as f64;
        // Rotation-invariant bounding sphere of the frustum slice.
        let (d, radius) = if orthographic {
            let d = (slice_near + slice_far) * 0.5;
            let r = (tx * tx + ty * ty + ((slice_far - slice_near) * 0.5).powi(2)).sqrt();
            (d, r)
        } else {
            let mut d = (slice_near + slice_far) * (s + 1.0) * 0.5;
            if d > slice_far {
                d = slice_far;
            }
            let r = (slice_far * slice_far * s + (slice_far - d) * (slice_far - d)).sqrt();
            (d, r)
        };
        // Quantize the radius so the texel size is stable.
        let radius = (radius * 16.0).ceil() / 16.0;
        let texel = 2.0 * radius / map_size as f64;

        // Snap the center to the texel grid in light space, in absolute world coordinates.
        let center_world = origin + forward * d;
        let mut ls = light_rot_inv * center_world;
        ls.x = (ls.x / texel).floor() * texel;
        ls.y = (ls.y / texel).floor() * texel;
        let center_rel = (light_rot * ls - origin).as_vec3();

        // Eye sits behind the sphere toward the sun; depth covers the sphere plus extra room.
        let back = radius as f32 * 2.0 + 50.0;
        let eye = center_rel + sun_dir * back;
        let light_view =
            Mat4::from_quat(light_rot.as_quat().inverse()) * Mat4::from_translation(-eye);
        let depth = back + radius as f32;
        let light_proj = crate::frame::orthographic_standard(radius as f32, 0.0, depth);
        out.light_view[i] = light_view;
        out.view_proj[i] = light_proj * light_view;
        out.half_extent[i] = radius as f32;
        out.depth_range[i] = depth;
        slice_near = slice_far;
    }
    out
}

fn look_rotation_f64(forward: DVec3) -> DQuat {
    let f = forward.normalize();
    let up = if f.y.abs() > 0.999 {
        DVec3::Z
    } else {
        DVec3::Y
    };
    let right = f.cross(up).normalize();
    let up = right.cross(f);
    DQuat::from_mat3(&DMat3::from_cols(right, up, -f))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_increase_to_far() {
        let s = split_distances(1.0, 300.0, 4, 0.8);
        assert!(s[0] < s[1] && s[1] < s[2] && s[2] < s[3]);
        assert!((s[3] - 300.0).abs() < 1e-3);
    }

    #[test]
    fn cascades_are_stable_under_rotation() {
        let settings = RenderSettings::default();
        let sun = Vec3::new(0.3, 0.8, 0.2).normalize();
        let origin = DVec3::new(1000.0, 20.0, -5000.0);
        let proj = crate::frame::perspective_infinite_reverse(1.2, 1.6, 0.1);
        let a = compute_cascades(
            &settings,
            origin,
            Quat::IDENTITY,
            0.1,
            1.2,
            1.6,
            false,
            sun,
            &proj,
        );
        let b = compute_cascades(
            &settings,
            origin,
            Quat::from_rotation_y(0.3),
            0.1,
            1.2,
            1.6,
            false,
            sun,
            &proj,
        );
        // Same size regardless of where the camera looks.
        for i in 0..MAX_CASCADES {
            assert_eq!(a.half_extent[i], b.half_extent[i]);
        }
        // A point at the camera is inside cascade 0.
        assert!(a.sphere_in_cascade(0, Vec3::ZERO, 0.5));
        // A point far away is not in cascade 0.
        assert!(!a.sphere_in_cascade(0, Vec3::new(0.0, 0.0, 2000.0), 1.0));
        // Projecting a point near the camera lands inside clip space.
        let c = a.view_proj[0] * Vec4::new(0.0, 0.0, -1.0, 1.0);
        assert!(c.x.abs() <= 1.0 && c.y.abs() <= 1.0 && (0.0..=1.0).contains(&c.z));
    }
}

#[cfg(test)]
mod far_shadow_tests {
    use super::*;

    #[test]
    fn far_view_covers_kilometers_and_keeps_casters_toward_the_sun() {
        let sun = Vec3::new(0.0, 0.2, -1.0).normalize();
        let v = FarShadowView::new(DVec3::new(1234.5, 30.0, -77.0), sun, 2048, 8000.0, 400.0);
        assert!(v.enabled);
        assert!((v.texel() - 16000.0 / 2048.0).abs() < 1e-3);
        // Ground 2 km in front of the camera projects inside the map, with a valid depth.
        let c = v.view_proj * Vec4::new(300.0, 0.0, 2000.0, 1.0);
        assert!(c.x.abs() < 1.0 && c.y.abs() < 1.0 && (0.0..1.0).contains(&c.z));
        // A mountain 6 km toward the sun is kept (it shades the valley), a pebble is not.
        assert!(v.casts(Vec3::new(0.0, 800.0, -6000.0), 1500.0));
        assert!(!v.casts(Vec3::new(0.0, 0.0, -100.0), 0.5));
        // Far beside the map: not kept.
        assert!(!v.casts(Vec3::new(30_000.0, 0.0, 0.0), 500.0));
        // The cascades already reach far enough: off.
        assert!(!FarShadowView::new(DVec3::ZERO, sun, 2048, 300.0, 400.0).enabled);
    }

    #[test]
    fn far_view_is_snapped_to_texels() {
        let sun = Vec3::new(0.3, 0.6, 0.2).normalize();
        let a = FarShadowView::new(DVec3::new(100.0, 0.0, 0.0), sun, 1024, 6000.0, 400.0);
        let b = FarShadowView::new(DVec3::new(100.3, 0.0, 0.1), sun, 1024, 6000.0, 400.0);
        // The same world point lands on the same texel position (up to the camera's own move).
        let world = DVec3::new(2500.0, 120.0, -900.0);
        let pa = a.view_proj * (world - DVec3::new(100.0, 0.0, 0.0)).as_vec3().extend(1.0);
        let pb = b.view_proj * (world - DVec3::new(100.3, 0.0, 0.1)).as_vec3().extend(1.0);
        let texel_ndc = 2.0 / 1024.0;
        let frac = |x: f32| ((x + 1.0) / texel_ndc).fract();
        assert!((frac(pa.x) - frac(pb.x)).abs() < 0.02 || (frac(pa.x) - frac(pb.x)).abs() > 0.98);
    }
}

#[cfg(test)]
mod sky_occlusion_tests {
    use super::*;

    #[test]
    fn sky_occlusion_maps_points_below_the_eye_into_the_unit_square() {
        let v = SkyOcclusionView::new(DVec3::new(1234.5, 10.0, -77.25), 512, 256.0, 800.0, true);
        // A point 3 m above the camera's feet, 20 m away: inside the square, depth in 0..1.
        let clip = v.view_proj * Vec4::new(20.0, 3.0, -15.0, 1.0);
        assert!(clip.x.abs() < 1.0 && clip.y.abs() < 1.0);
        assert!(clip.z > 0.0 && clip.z < 1.0);
        // Higher points have smaller depth (closer to the eye above).
        let higher = v.view_proj * Vec4::new(20.0, 30.0, -15.0, 1.0);
        assert!(higher.z < clip.z);
        assert!(v.contains_sphere(Vec3::new(20.0, 3.0, -15.0), 1.0));
        assert!(!v.contains_sphere(Vec3::new(500.0, 3.0, 0.0), 1.0));
    }
}
