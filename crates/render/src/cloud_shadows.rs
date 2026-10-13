//! Cloud shadows: a top-down map of the sunlight the clouds let through.
//!
//! Whatever draws clouds (the weather module) asks for the map from its [`RenderFeature`] with
//! [`RenderFeature::cloud_shadows`] and draws it during [`RenderFeature::compute`], into
//! [`FrameContext::cloud_shadows`](crate::FrameContext::cloud_shadows). Everything the sun
//! lights then multiplies the sunlight by it: `shade_surface` and `shade_thin_surface` (so
//! every mesh, grass, and the terrain, water and foliage built on them), the grass
//! translucency, the volumetric fog's sunlight and the height fog's sun glow beyond it
//! (`apply_fog`). Other crates' shaders use `cloud_shadow(rel)` from `lib_frame.wgsl`.
//!
//! **The area** is requested by the writer ([`CloudShadowRequest`]): how many texels, how many
//! meters across, and the heights of the cloud layer. The renderer places the square around
//! the camera every frame, snapped to whole texels in world space ([`CloudShadowArea::place`]),
//! so a texel always covers the same piece of the world and nothing swims when the camera
//! moves.
//!
//! **What a texel holds**: red is the share of sunlight (0..1) that comes down through the
//! clouds to the point on the cloud base plane (world height `base_height`) at the texel's
//! center, along the sun direction. Shading looks the map up where the sun ray through the
//! shaded point crosses that plane, so one map is right for every point below the clouds, a
//! valley floor and a mountain top alike, and the shadows fall offset away from the sun as
//! they should. Between `base_height` and `top_height` the shadow fades out, and above the
//! layer nothing is shaded. Beyond the map's square the sunlight is `outside` (the map fades
//! into it over its outer tenth). Green, blue and alpha are not read by the renderer: they are
//! free for other top-down weather data a module wants to hand its own shaders (rain shafts,
//! say), without another binding. The map is cleared to 1 at the start of every frame, so
//! when nobody draws it (no weather module, or one that skips a frame) nothing is shaded and
//! every image is exactly as without it.
//!
//! **Texel rows** run along +z and columns along +x: texel (x, y) covers world x from
//! `min.x + x * texel` and z from `min.y + y * texel`. A fullscreen triangle drawn into the
//! view (`fullscreen_vertex`) has `uv` = (u along x, v along z), and `cloud_shadow_plane_point(uv)`
//! in `lib_frame.wgsl` gives the camera-relative point on the base plane there, from which a
//! writer marches toward `frame.sun_dir`. To rasterize cloud shapes instead, use
//! `frame.cloud_shadow` (camera-relative position → the map's clip space, projected along the
//! sun onto the base plane). Writers can also use a compute shader: the texture is a storage
//! texture too.
//!
//! The map is bound at frame binding 21 (`cloud_shadow_map`). While features run `compute` the
//! frame group holds a white stand-in there instead, so a writer can bind `ctx.frame_group` in
//! the pass that draws the map (and `cloud_shadow` returns 1 in compute passes).

use bytemuck::{Pod, Zeroable};
use engine_core::prelude::*;

/// Format of the cloud shadow map: renderable, filterable and usable as a storage texture.
/// Red is the sun transmittance; green, blue and alpha are free for the writer.
pub const CLOUD_SHADOW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// Fewest texels per side of the cloud shadow map.
pub const MIN_CLOUD_SHADOW_SIZE: u32 = 16;
/// Most texels per side of the cloud shadow map.
pub const MAX_CLOUD_SHADOW_SIZE: u32 = 2048;

/// The sun's height above the horizon used for the projection never goes below this (the sine
/// of about 3°): at sunset the shadows stretch at most 20 times the height of the clouds above
/// the ground, and the sun is too dim for it to matter beyond.
const MIN_SUN_HEIGHT: f32 = 0.05;

/// The cloud shadow map a feature asks for ([`RenderFeature::cloud_shadows`]).
///
/// The weather module's clouds: `CloudShadowRequest { size: 128, extent: 128_000.0,
/// base_height: 1500.0, top_height: 4000.0, outside: 0.6 }` gives 1 km texels over 128 km.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CloudShadowRequest {
    /// Texels per side (16..=2048).
    pub size: u32,
    /// Side of the square the map covers, meters. It is centered on the camera and snapped to
    /// whole texels.
    pub extent: f32,
    /// World height (y) of the cloud base, meters: the plane the map lies in. The map holds
    /// the sunlight that reaches this plane; points below it get the value where their sun ray
    /// crosses it.
    pub base_height: f64,
    /// World height (y) of the cloud tops, meters. Between the base and the tops the shadow
    /// fades out (points inside the clouds); above them nothing is shaded.
    pub top_height: f64,
    /// Sun transmittance (0..1) beyond the square, such as the weather's average there; the
    /// map fades into it over its outer tenth.
    pub outside: f32,
}

impl Default for CloudShadowRequest {
    fn default() -> Self {
        Self {
            size: 128,
            extent: 128_000.0,
            base_height: 1500.0,
            top_height: 4000.0,
            outside: 1.0,
        }
    }
}

/// Where this frame's cloud shadow map lies in the world: a [`CloudShadowRequest`] placed
/// around the camera ([`CloudShadowArea::place`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CloudShadowArea {
    /// World x (`.x`) and z (`.y`) of the map's corner at uv (0, 0): its smallest x and z.
    pub min: DVec2,
    /// Side of the square, meters.
    pub extent: f32,
    /// Texels per side.
    pub size: u32,
    /// World height of the base plane, meters.
    pub base_height: f64,
    /// World height of the cloud tops, meters (at least `base_height`).
    pub top_height: f64,
    /// Sun transmittance beyond the square (0..1).
    pub outside: f32,
    /// Direction toward the sun this frame (unit), along which points are projected onto the
    /// base plane.
    pub sun_dir: Vec3,
}

impl CloudShadowArea {
    /// Place `request` around a camera at world position `camera` for a sun in direction
    /// `sun_dir` (toward the sun). The square's corner is snapped to whole texels in world
    /// space, so texels stay put while the camera moves.
    pub fn place(request: &CloudShadowRequest, camera: DVec3, sun_dir: Vec3) -> Self {
        let size = request
            .size
            .clamp(MIN_CLOUD_SHADOW_SIZE, MAX_CLOUD_SHADOW_SIZE);
        let extent = if request.extent.is_finite() {
            request.extent.max(1.0)
        } else {
            128_000.0
        };
        let texel = extent as f64 / size as f64;
        let half = extent as f64 * 0.5;
        let min = DVec2::new(
            ((camera.x - half) / texel).round() * texel,
            ((camera.z - half) / texel).round() * texel,
        );
        let base_height = if request.base_height.is_finite() {
            request.base_height
        } else {
            0.0
        };
        let top_height = if request.top_height.is_finite() {
            request.top_height.max(base_height)
        } else {
            base_height
        };
        Self {
            min,
            extent,
            size,
            base_height,
            top_height,
            outside: if request.outside.is_finite() {
                request.outside.clamp(0.0, 1.0)
            } else {
                1.0
            },
            sun_dir: sun_dir.normalize_or(Vec3::Y),
        }
    }

    /// Size of one texel, meters.
    pub fn texel_size(&self) -> f64 {
        self.extent as f64 / self.size.max(1) as f64
    }

    /// World position of the center of texel (`x`, `y`) on the base plane (`x` along world x,
    /// `y` along world z).
    pub fn texel_center(&self, x: u32, y: u32) -> DVec3 {
        let t = self.texel_size();
        DVec3::new(
            self.min.x + (x as f64 + 0.5) * t,
            self.base_height,
            self.min.y + (y as f64 + 0.5) * t,
        )
    }

    /// How far a point moves across the base plane per meter it lies below it (x and z): the
    /// sun ray's horizontal run per meter of rise.
    pub fn sun_slope(&self) -> DVec2 {
        let l = self.sun_dir.as_dvec3();
        DVec2::new(l.x, l.z) / l.y.max(MIN_SUN_HEIGHT as f64)
    }

    /// Map coordinates (0..1 inside the square; u along x, v along z) of the point where the
    /// sun ray through world point `p` crosses the base plane: where shading reads the map for
    /// `p`.
    pub fn uv(&self, p: DVec3) -> DVec2 {
        let q = DVec2::new(p.x, p.z) + self.sun_slope() * (self.base_height - p.y);
        (q - self.min) / self.extent as f64
    }

    /// The frame uniform's cloud shadow fields for a camera at world position `camera`.
    pub(crate) fn uniform(&self, camera: DVec3) -> GpuCloudShadow {
        let e = self.extent as f64;
        let slope = self.sun_slope();
        let base_rel = self.base_height - camera.y;
        let min_rel = self.min - DVec2::new(camera.x, camera.z);
        // Camera-relative r → base plane crossing q = r.xz + slope * (base_rel - r.y)
        // → clip x = 2u - 1, y = 1 - 2v (rows run along +z, like the texture's).
        let k = 2.0 / e;
        let to_map = Mat4::from_cols(
            Vec4::new(k as f32, 0.0, 0.0, 0.0),
            Vec4::new((-k * slope.x) as f32, (k * slope.y) as f32, 0.0, 0.0),
            Vec4::new(0.0, -k as f32, 0.0, 0.0),
            Vec4::new(
                (k * (slope.x * base_rel - min_rel.x) - 1.0) as f32,
                (1.0 - k * (slope.y * base_rel - min_rel.y)) as f32,
                0.0,
                1.0,
            ),
        );
        GpuCloudShadow {
            to_map: to_map.to_cols_array_2d(),
            params: [
                1.0,
                base_rel as f32,
                (self.top_height - self.base_height) as f32,
                self.outside,
            ],
            area: [
                min_rel.x as f32,
                min_rel.y as f32,
                self.extent,
                self.size as f32,
            ],
        }
    }
}

/// The frame uniform's cloud shadow fields (`cloud_shadow`, `cloud_shadow_params`,
/// `cloud_shadow_area` in `lib_types.wgsl`).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, Pod, Zeroable)]
pub struct GpuCloudShadow {
    /// Camera-relative position → map clip space (x, y in −1..1 across the square), projected
    /// along the sun onto the base plane.
    pub to_map: [[f32; 4]; 4],
    /// Drawn this frame (1/0), base plane height relative to the camera (m), layer thickness
    /// (m), transmittance beyond the square.
    pub params: [f32; 4],
    /// Camera-relative x and z of the corner at uv (0, 0) (m), side (m), texels per side.
    pub area: [f32; 4],
}

/// The cloud shadow map to draw into during [`RenderFeature::compute`]: given to the feature
/// whose [`RenderFeature::cloud_shadows`] asked for it.
#[derive(Clone, Copy)]
pub struct CloudShadowTarget<'a> {
    /// The map (`size`² texels of [`CLOUD_SHADOW_FORMAT`]; render attachment, storage and
    /// sampled texture), already cleared to 1.
    pub view: &'a wgpu::TextureView,
    /// Its format, [`CLOUD_SHADOW_FORMAT`].
    pub format: wgpu::TextureFormat,
    /// Where it lies this frame.
    pub area: CloudShadowArea,
}

/// The texture behind frame binding 21, and the white stand-in compute passes see there.
pub(crate) struct CloudShadowMap {
    pub view: wgpu::TextureView,
    pub size: u32,
    pub stand_in: wgpu::TextureView,
}

impl CloudShadowMap {
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            view: create_map(device, "cloud shadows", 1),
            size: 1,
            stand_in: create_map(device, "no cloud shadows", 1),
        }
    }

    /// Recreate the map at `size`² texels if it has another size. Returns whether it was
    /// recreated (rebuild the bind groups).
    pub fn ensure_size(&mut self, device: &wgpu::Device, size: u32) -> bool {
        if size == self.size {
            return false;
        }
        self.view = create_map(device, "cloud shadows", size);
        self.size = size;
        true
    }

    /// Clear the map to 1 (no shadow). With `stand_in`, the stand-in too (once, after it is
    /// created: new textures start at 0).
    pub fn clear(&self, encoder: &mut wgpu::CommandEncoder, stand_in: bool) {
        let views = [Some(&self.view), stand_in.then_some(&self.stand_in)];
        for view in views.into_iter().flatten() {
            encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("clear cloud shadows"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
        }
    }
}

fn create_map(device: &wgpu::Device, label: &str, size: u32) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: size.max(1),
                height: size.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: CLOUD_SHADOW_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(camera: DVec3, sun: Vec3) -> CloudShadowArea {
        CloudShadowArea::place(
            &CloudShadowRequest {
                size: 128,
                extent: 128_000.0,
                base_height: 1500.0,
                top_height: 3000.0,
                outside: 0.5,
            },
            camera,
            sun,
        )
    }

    /// What the shader computes: clip space from the uniform's matrix, then uv.
    fn shader_uv(u: &GpuCloudShadow, rel: Vec3) -> Vec2 {
        let c = Mat4::from_cols_array_2d(&u.to_map) * rel.extend(1.0);
        Vec2::new(c.x * 0.5 + 0.5, 0.5 - c.y * 0.5)
    }

    #[test]
    fn texels_stay_put_in_the_world_as_the_camera_moves() {
        let sun = Vec3::new(0.3, 0.8, -0.2).normalize();
        let a = area(DVec3::new(10_250.3, 40.0, -3_100.7), sun);
        let b = area(DVec3::new(10_731.9, 52.0, -2_480.2), sun);
        let t = a.texel_size();
        assert_eq!(t, 1000.0);
        // Both corners lie on the same world grid of texels.
        for m in [a.min, b.min] {
            assert!((m.x / t - (m.x / t).round()).abs() < 1e-9, "{m}");
            assert!((m.y / t - (m.y / t).round()).abs() < 1e-9, "{m}");
        }
        // A world point maps to the same place in the world through either placement.
        let p = DVec3::new(9_876.5, 3.0, -2_222.2);
        let wa = a.min + a.uv(p) * a.extent as f64;
        let wb = b.min + b.uv(p) * b.extent as f64;
        assert!((wa - wb).length() < 1e-6, "{wa} vs {wb}");
    }

    #[test]
    fn the_uniform_matches_the_cpu_mapping() {
        let sun = Vec3::new(-0.4, 0.6, 0.3).normalize();
        let camera = DVec3::new(123_456.7, 812.0, -98_765.4);
        let a = area(camera, sun);
        let u = a.uniform(camera);
        for p in [
            DVec3::new(123_400.0, 0.0, -98_700.0),
            DVec3::new(150_000.0, 1200.0, -70_000.0),
            DVec3::new(100_000.0, -50.0, -120_000.0),
        ] {
            let want = a.uv(p).as_vec2();
            let got = shader_uv(&u, (p - camera).as_vec3());
            assert!((want - got).length() < 1e-5, "{p}: {want} vs {got}");
        }
        // The point on the base plane at a texel's center reads that texel's center.
        let c = a.texel_center(17, 99);
        let uv = a.uv(c);
        assert!(
            (uv * 128.0 - DVec2::new(17.5, 99.5)).length() < 1e-6,
            "{uv}"
        );
        // And the writer's inverse (`cloud_shadow_plane_point`) finds it again.
        let rel = Vec3::new(
            u.area[0] + uv.x as f32 * u.area[2],
            u.params[1],
            u.area[1] + uv.y as f32 * u.area[2],
        );
        assert!(((c - camera).as_vec3() - rel).length() < 0.05, "{rel}");
    }

    #[test]
    fn shadows_fall_away_from_the_sun() {
        // Sun in the +x direction, 45° up: a point 1500 m below the base reads the map 1500 m
        // toward the sun.
        let sun = Vec3::new(1.0, 1.0, 0.0).normalize();
        let a = area(DVec3::ZERO, sun);
        let below = a.uv(DVec3::new(0.0, 0.0, 0.0));
        let on = a.uv(DVec3::new(1500.0, 1500.0, 0.0));
        assert!((below - on).length() < 1e-9, "{below} vs {on}");
        // A sun below the horizon doesn't send the lookup to infinity.
        let low = area(DVec3::ZERO, Vec3::new(1.0, -0.2, 0.0));
        assert!(low.uv(DVec3::ZERO).x.is_finite());
    }

    #[test]
    fn requests_are_sanitized() {
        let a = CloudShadowArea::place(
            &CloudShadowRequest {
                size: 1,
                extent: f32::NAN,
                base_height: 100.0,
                top_height: 50.0,
                outside: 7.0,
            },
            DVec3::ZERO,
            Vec3::ZERO,
        );
        assert_eq!(a.size, MIN_CLOUD_SHADOW_SIZE);
        assert!(a.extent.is_finite() && a.extent > 0.0);
        assert_eq!(a.top_height, 100.0);
        assert_eq!(a.outside, 1.0);
        assert_eq!(a.sun_dir, Vec3::Y);
    }
}
