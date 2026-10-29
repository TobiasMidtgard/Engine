//! Planar reflections: a flat mirror (a calm lake, a pool, a polished floor) shows the scene
//! reflected in its plane, sharp to the edge of the screen, with what is off screen and what
//! faces away from the camera (a boat's underside, a mountain behind the camera's top edge),
//! which screen-space reflections can't show.
//!
//! Each [`PlanarReflection`] the main camera sees from above gets an extra view
//! ([`crate::ExtraView`]) whose camera the renderer places every frame, just before the extra
//! views render ([`place_reflection_views`]), as Unreal's planar reflections and HDRP's planar
//! reflection probe do:
//!
//! - the main camera's eye mirrored in the plane, looking along the mirrored view, turned as a
//!   proper camera (its right axis flipped), so the meshes' winding and culling stay as they
//!   are and the picture comes out mirrored left to right, which is what a mirror shows;
//! - an off-axis frustum around just the part of the screen the mirror covers and a margin
//!   ([`MARGIN`], for rays the waves tilt) (Kooima, as portals use), so a lake in the bottom
//!   third of the screen gets all of the picture;
//! - its near plane as far out as the plane allows (the nearest point of the plane the frustum
//!   sees), so nothing between the mirrored eye and the plane is drawn near it. Further out the
//!   picture can still hold what lies under the plane: users compare the picture's depth with
//!   the plane's (`PlanarReflectionPicture::near`) and drop those hits, as Crest and Unreal sink
//!   the clip plane. A true oblique near plane (Lengyel) would break the infinite reverse-Z
//!   projection that sky, fog and the screen effects rely on (`engine-design/portals.md`).
//!
//! The picture is the view's HDR scene color (after its anti-aliasing, before bloom and
//! tonemapping) copied into a mip chain, each level half the size of the one above, for rough
//! reflections (HDRP's "rough reflections"), with the view's depth beside it
//! ([`PlanarReflectionPicture`]). Features sample it for the plane's points by projecting them
//! with [`PlanarReflectionPicture::view_proj`]: a point of the plane lands exactly where the
//! mirror shows it from the main camera. While one renders, [`rendering_reflection`] says which
//! mirror's view is drawing, so the mirror's own surface (the water) can stay out of it.

use std::collections::HashMap;

use engine_core::prelude::*;

use crate::components::{Camera, ExtraView, Projection};
use crate::frame::{RenderingExtraView, perspective_infinite_reverse};

/// A flat mirror in this entity's XZ plane (its local +Y is the side it reflects), `size`
/// meters across (X by Z, scaled by the transform), centered on the entity.
///
/// Costs an extra view of the scene while the main camera sees it from above, at about the
/// pixels it covers on screen times `resolution`.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[require(Transform)]
#[serde(default)]
pub struct PlanarReflection {
    /// Width (local X) and depth (local Z), meters.
    pub size: Vec2,
    /// The picture's resolution against the pixels the mirror covers on screen (0.05..2).
    pub resolution: f32,
    /// Quality of the reflected view instead of the game's (`Scalability`).
    pub quality: Option<engine_world::Scalability>,
}

impl Default for PlanarReflection {
    fn default() -> Self {
        Self {
            size: Vec2::splat(10.0),
            resolution: 0.5,
            quality: None,
        }
    }
}

impl PlanarReflection {
    pub fn new(size: Vec2) -> Self {
        Self {
            size,
            ..Default::default()
        }
    }

    pub fn with_resolution(mut self, resolution: f32) -> Self {
        self.resolution = resolution;
        self
    }

    pub fn with_quality(mut self, quality: engine_world::Scalability) -> Self {
        self.quality = Some(quality);
        self
    }
}

/// The picture reaches this far beyond the part of the screen the mirror covers, as a share of
/// the screen's width and height on each side.
pub const MARGIN: f32 = 0.08;

/// On a mirror's view camera (spawned by [`place_reflection_views`]): its off-axis frustum,
/// which `build_view` uses instead of the camera's projection, and its picture's size.
#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct MirrorCamera {
    pub mirror: Entity,
    /// Left, right, bottom, top edges of the frustum as tangents (x / -z, y / -z).
    pub tangents: Vec4,
    /// Near plane, meters: the nearest the plane comes inside the frustum.
    pub near: f32,
    /// Picture size, pixels.
    pub size: UVec2,
    /// The mirror's plane: a point on it and its normal (the reflecting side).
    pub plane_point: DVec3,
    pub plane_normal: Vec3,
}

/// What lies this far under a mirror's plane still shows in its view (meters), so the
/// waterline of things standing in it meets their reflection without a seam (Crest sinks the
/// plane the same way).
const CLIP_SINK_M: f64 = 0.03;

impl MirrorCamera {
    /// The plane its view cuts away what lies under, relative to a view at `origin`: (n, d)
    /// with `dot(n, rel) + d < 0` cut.
    pub fn clip_plane(&self, origin: DVec3) -> [f32; 4] {
        let n = self.plane_normal.as_dvec3();
        let d = -n.dot(self.plane_point - origin) + CLIP_SINK_M;
        [n.x as f32, n.y as f32, n.z as f32, d as f32]
    }
}

/// The view camera of each mirror.
#[derive(Resource, Default)]
pub(crate) struct MirrorViews(HashMap<Entity, Entity>);

/// What a mirror's view rendered: its HDR scene color with a mip chain and its depth, with the
/// projection and exposure they were made with. See [`planar_reflection_picture`].
#[derive(Debug, Clone)]
pub struct PlanarReflectionPicture {
    /// The view's camera.
    pub camera: Entity,
    /// HDR scene color, pre-exposed by `exposure` (multiply by the drawing view's exposure over
    /// it), with `mips` levels, each a 2×2 box filter of the one above.
    pub color: wgpu::TextureView,
    /// The view's depth (reverse-Z: `near / view depth`, 0 for the sky), the size of level 0.
    pub depth: wgpu::TextureView,
    pub mips: u32,
    /// Picture size, pixels (level 0).
    pub size: UVec2,
    /// Unjittered view-projection of the picture, relative to `origin` (its eye).
    pub view_proj: Mat4,
    /// The mirrored eye.
    pub origin: DVec3,
    /// Near plane, meters (depth 1 there).
    pub near: f32,
    /// The view's pre-exposure.
    pub exposure: f32,
    /// The mirror's plane: a point on it and its normal (the reflecting side).
    pub plane_point: DVec3,
    pub plane_normal: Vec3,
}

impl PlanarReflectionPicture {
    /// `view_proj` for positions relative to `origin` (another view's eye): what a feature
    /// drawing in that view projects its camera-relative points with.
    pub fn view_proj_from(&self, origin: DVec3) -> Mat4 {
        let shift = (origin - self.origin).as_vec3();
        self.view_proj * Mat4::from_translation(shift)
    }
}

/// Each mirror's picture, and the mip-chained copies they are made into.
#[derive(Resource, Default)]
pub(crate) struct MirrorPictures {
    pub pictures: HashMap<Entity, PlanarReflectionPicture>,
    pub chains: HashMap<Entity, wgpu::Texture>,
}

/// The picture of the mirror `mirror` (an entity with a [`PlanarReflection`]) from this
/// frame's view of it, for features drawing the main view: `None` while the main camera
/// doesn't see the mirror from above.
pub fn planar_reflection_picture(
    world: &World,
    mirror: Entity,
) -> Option<&PlanarReflectionPicture> {
    world
        .get_resource::<MirrorPictures>()?
        .pictures
        .get(&mirror)
}

/// The mirror whose reflected view is rendering now, if any (features leave the mirror's own
/// surface out of it).
pub fn rendering_reflection(world: &World) -> Option<Entity> {
    let RenderingExtraView(camera) = *world.get_resource::<RenderingExtraView>()?;
    world.get::<MirrorCamera>(camera).map(|m| m.mirror)
}

/// Where a mirror's view camera goes this frame.
#[derive(Debug, Clone, Copy)]
struct Placement {
    transform: GlobalTransform,
    tangents: Vec4,
    near: f32,
    /// The part of the screen the mirror covers, pixels.
    pixels: Vec2,
}

/// `v` mirrored in the plane through the origin with unit normal `n`.
fn reflect(v: DVec3, n: DVec3) -> DVec3 {
    v - n * (2.0 * v.dot(n))
}

/// Where the view of the mirror (at `at`) is seen from for a camera `camera` at `eye` with a
/// `viewport`-sized picture: `None` when the camera is below the plane or doesn't see the
/// mirror.
fn place(
    mirror: &PlanarReflection,
    at: &GlobalTransform,
    camera: &Camera,
    eye: &GlobalTransform,
    viewport: UVec2,
) -> Option<Placement> {
    let Projection::Perspective { fov_y_deg, near } = camera.projection else {
        return None;
    };
    let scale = at.scale();
    let half = (mirror.size * Vec2::new(scale.x, scale.z)).abs() * 0.5;
    if half.min_element() <= 1e-4 {
        return None;
    }
    let rot = at.rotation().as_dquat();
    let n = (rot * DVec3::Y).normalize();
    let eye_pos = eye.translation();
    let height = (eye_pos - at.translation()).dot(n);
    if height <= 1e-3 {
        return None;
    }
    let viewport = viewport.max(UVec2::ONE).as_vec2();
    let aspect = viewport.x / viewport.y;
    let fov = fov_y_deg.clamp(1.0, 170.0).to_radians();
    let ty = (fov * 0.5).tan();
    let tx = ty * aspect;
    let view_proj = perspective_infinite_reverse(fov, aspect, near.max(1e-4))
        * Mat4::from_quat(eye.rotation().inverse());
    let center = (at.translation() - eye_pos).as_vec3();
    if !Frustum::from_view_proj(&view_proj).intersects_sphere(center, half.length()) {
        return None;
    }
    // The rectangle in the camera's view space (x right, y up, -z ahead), cut at the near
    // plane, and its bounds as tangents.
    let eye_rot = eye.rotation().as_dquat();
    let to_view = |p: DVec3| eye_rot.inverse() * (p - eye_pos);
    let corners = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)].map(|(sx, sz)| {
        to_view(at.translation() + rot * DVec3::new(sx * half.x as f64, 0.0, sz * half.y as f64))
    });
    let cut = -(near.max(1e-4) as f64);
    let mut polygon = Vec::with_capacity(8);
    for i in 0..4 {
        let (a, b) = (corners[i], corners[(i + 1) % 4]);
        if a.z <= cut {
            polygon.push(a);
        }
        if (a.z <= cut) != (b.z <= cut) {
            let t = (cut - a.z) / (b.z - a.z);
            polygon.push(a + (b - a) * t);
        }
    }
    if polygon.is_empty() {
        return None;
    }
    let (mut lo, mut hi) = (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY));
    for p in &polygon {
        let t = DVec2::new(p.x, p.y) / -p.z;
        lo = lo.min(t);
        hi = hi.max(t);
    }
    let full = DVec2::new(tx as f64, ty as f64);
    let (lo, hi) = (lo.max(-full), hi.min(full));
    if lo.x >= hi.x || lo.y >= hi.y {
        return None;
    }
    // A margin around it: rough or wavy surfaces look up a little off the flat mirror's
    // rays, and should still find the picture there.
    let margin = full * 2.0 * MARGIN as f64;
    let (lo, hi) = (lo - margin, hi + margin);
    // The nearest the plane comes inside that frustum: where its corner rays meet it.
    let n_view = eye_rot.inverse() * n;
    let mut nearest = f64::INFINITY;
    for (x, y) in [(lo.x, lo.y), (hi.x, lo.y), (hi.x, hi.y), (lo.x, hi.y)] {
        let along = DVec3::new(x, y, -1.0).dot(n_view);
        if along < -1e-9 {
            nearest = nearest.min(height / -along);
        }
    }
    if !nearest.is_finite() {
        return None;
    }
    let near_plane = (nearest * 0.999).max(near.max(1e-4) as f64) as f32;
    // The mirrored eye and view, as a proper camera: right flipped, so the picture is the
    // mirror image left to right (x' = -x, y' = y, depth the same for points of the plane).
    let (right, up, back) = (eye_rot * DVec3::X, eye_rot * DVec3::Y, eye_rot * DVec3::Z);
    let basis = DMat3::from_cols(-reflect(right, n), reflect(up, n), reflect(back, n));
    let mirrored_rot = DQuat::from_mat3(&basis).normalize();
    let origin = eye_pos - n * (2.0 * height);
    let tangents = Vec4::new(-hi.x as f32, -lo.x as f32, lo.y as f32, hi.y as f32);
    let pixels = Vec2::new(
        ((hi.x - lo.x) / (2.0 * full.x)) as f32 * viewport.x,
        ((hi.y - lo.y) / (2.0 * full.y)) as f32 * viewport.y,
    );
    Some(Placement {
        transform: GlobalTransform::from_translation_rotation(origin, mirrored_rot.as_quat()),
        tangents,
        near: near_plane,
        pixels,
    })
}

/// Place every mirror's view camera for the main camera: spawn it for a new mirror, place it
/// and size its picture, turn it off (and drop its picture) while the main camera is below
/// the mirror or doesn't see it, and drop it with its mirror. The renderer calls this before
/// the extra views render.
pub(crate) fn place_reflection_views(world: &mut World) {
    let mirrors: Vec<(Entity, PlanarReflection, GlobalTransform)> = world
        .query::<(Entity, &PlanarReflection, &GlobalTransform)>()
        .iter(world)
        .map(|(e, m, gt)| (e, *m, *gt))
        .collect();
    if mirrors.is_empty() && !world.contains_resource::<MirrorViews>() {
        return;
    }
    let mut views = world.remove_resource::<MirrorViews>().unwrap_or_default();
    let gone: Vec<(Entity, Entity)> = views
        .0
        .iter()
        .filter(|(mirror, _)| !mirrors.iter().any(|m| m.0 == **mirror))
        .map(|(m, c)| (*m, *c))
        .collect();
    for (mirror, camera) in gone {
        views.0.remove(&mirror);
        world.despawn(camera);
        if let Some(mut pictures) = world.get_resource_mut::<MirrorPictures>() {
            pictures.pictures.remove(&mirror);
            pictures.chains.remove(&mirror);
        }
    }
    let main = crate::frame::active_camera(world);
    let window = world
        .get_resource::<Window>()
        .map(|w| UVec2::new(w.width.max(1), w.height.max(1)))
        .unwrap_or(UVec2::ONE);
    let (_, viewport) = crate::frame::camera_viewport(world, window);
    for (entity, mirror, at) in mirrors {
        let placed = main.as_ref().and_then(|(_, camera, eye)| {
            place(&mirror, &at, camera, eye, viewport).map(|p| (p, *camera))
        });
        let camera = *views.0.entry(entity).or_insert_with(|| {
            world
                .spawn((
                    Camera {
                        active: false,
                        ..Default::default()
                    },
                    ExtraView::window(),
                    MirrorCamera {
                        mirror: entity,
                        tangents: Vec4::new(-1.0, 1.0, -1.0, 1.0),
                        near: 1.0,
                        size: UVec2::ZERO,
                        plane_point: at.translation(),
                        plane_normal: at.rotation() * Vec3::Y,
                    },
                ))
                .id()
        });
        let Ok(mut e) = world.get_entity_mut(camera) else {
            views.0.remove(&entity);
            continue;
        };
        match placed {
            Some((placement, main_camera)) => {
                let previous = e.get::<MirrorCamera>().map_or(UVec2::ZERO, |m| m.size);
                let size =
                    crate::portal::picture_size(placement.pixels, mirror.resolution, previous);
                e.insert((
                    Camera {
                        projection: Projection::Perspective {
                            fov_y_deg: 60.0,
                            near: placement.near,
                        },
                        exposure: main_camera.exposure,
                        active: true,
                        order: main_camera.order.saturating_sub(1),
                    },
                    placement.transform.to_transform(),
                    placement.transform,
                    MirrorCamera {
                        mirror: entity,
                        tangents: placement.tangents,
                        near: placement.near,
                        size,
                        plane_point: at.translation(),
                        plane_normal: (at.rotation() * Vec3::Y).normalize(),
                    },
                ));
                if e.get::<ExtraView>()
                    .is_none_or(|x| x.quality != mirror.quality)
                {
                    e.insert(ExtraView {
                        quality: mirror.quality,
                        ..ExtraView::window()
                    });
                }
            }
            None => {
                if let Some(mut c) = e.get_mut::<Camera>()
                    && c.active
                {
                    c.active = false;
                }
                if let Some(mut pictures) = world.get_resource_mut::<MirrorPictures>() {
                    pictures.pictures.remove(&entity);
                }
            }
        }
    }
    world.insert_resource(views);
}

/// Mip levels of a picture `size` pixels: down to about 8 pixels on its short side, at most 6.
pub(crate) fn picture_mips(size: UVec2) -> u32 {
    let short = size.min_element().max(1);
    (32 - (short / 8).max(1).leading_zeros()).clamp(1, 6)
}

/// Copy a mirror view's HDR scene color into its mip chain (made or remade at `size`) and
/// filter the levels, then keep the picture for features to sample: the view's unjittered
/// view-projection, eye and exposure. `encoder` is submitted right after the view.
#[allow(clippy::too_many_arguments)]
pub(crate) fn keep_picture(
    world: &mut World,
    gpu: &crate::features::GpuShared,
    encoder: &mut wgpu::CommandEncoder,
    mirror: &MirrorCamera,
    camera: Entity,
    hdr: &wgpu::Texture,
    depth: &wgpu::TextureView,
    (view_proj, origin, exposure): (Mat4, DVec3, f32),
) {
    let size = UVec2::new(hdr.width(), hdr.height());
    let mips = picture_mips(size);
    let format = hdr.format();
    let mut pictures = world
        .remove_resource::<MirrorPictures>()
        .unwrap_or_default();
    let chain = pictures
        .chains
        .entry(mirror.mirror)
        .or_insert_with(|| make_chain(gpu, size, mips, format));
    if chain.width() != size.x || chain.height() != size.y || chain.mip_level_count() != mips {
        *chain = make_chain(gpu, size, mips, format);
    }
    encoder.copy_texture_to_texture(
        hdr.as_image_copy(),
        chain.as_image_copy(),
        wgpu::Extent3d {
            width: size.x,
            height: size.y,
            depth_or_array_layers: 1,
        },
    );
    gpu.mips.record(gpu, encoder, chain, format);
    let (plane_point, plane_normal) = world
        .get::<GlobalTransform>(mirror.mirror)
        .map_or((DVec3::ZERO, Vec3::Y), |gt| {
            (gt.translation(), (gt.rotation() * Vec3::Y).normalize())
        });
    let picture = PlanarReflectionPicture {
        camera,
        color: chain.create_view(&Default::default()),
        depth: depth.clone(),
        mips,
        size,
        view_proj,
        origin,
        near: mirror.near,
        exposure,
        plane_point,
        plane_normal,
    };
    pictures.pictures.insert(mirror.mirror, picture);
    world.insert_resource(pictures);
}

fn make_chain(
    gpu: &crate::features::GpuShared,
    size: UVec2,
    mips: u32,
    format: wgpu::TextureFormat,
) -> wgpu::Texture {
    gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("planar reflection"),
        size: wgpu::Extent3d {
            width: size.x.max(1),
            height: size.y.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: mips,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn looking(eye: DVec3, target: DVec3) -> GlobalTransform {
        let t = Transform::from_translation(eye).looking_at(target, Vec3::Y);
        GlobalTransform::from_translation_rotation(eye, t.rotation)
    }

    /// Where the view `placement` makes puts world point `p`: tangents (x / -z, y / -z) and
    /// view depth.
    fn seen(placement: &Placement, p: DVec3) -> (DVec2, f64) {
        let v = placement.transform.rotation().as_dquat().inverse()
            * (p - placement.transform.translation());
        (DVec2::new(v.x, v.y) / -v.z, -v.z)
    }

    #[test]
    fn the_view_is_the_camera_mirrored_in_the_plane() {
        let mirror = PlanarReflection::new(Vec2::new(40.0, 40.0));
        let at =
            GlobalTransform::from_translation_rotation(DVec3::new(0.0, 2.0, 0.0), Quat::IDENTITY);
        let eye = looking(DVec3::new(3.0, 5.0, 12.0), DVec3::new(-2.0, 2.0, -6.0));
        let viewport = UVec2::new(800, 450);
        let p = place(&mirror, &at, &Camera::perspective(60.0), &eye, viewport).expect("seen");
        // The eye 3 m above the plane, mirrored 3 m below it.
        let o = p.transform.translation();
        assert!(o.distance(DVec3::new(3.0, -1.0, 12.0)) < 1e-9, "{o}");
        // A point of the plane shows where the camera sees it, flipped left to right, at the
        // same depth: so a feature finds the reflection of a plane point by projecting it.
        let view = |q: DVec3| {
            let v = eye.rotation().as_dquat().inverse() * (q - eye.translation());
            (DVec2::new(v.x, v.y) / -v.z, -v.z)
        };
        for q in [DVec3::new(-1.0, 2.0, 2.0), DVec3::new(4.0, 2.0, -5.0)] {
            let (a, da) = view(q);
            let (b, db) = seen(&p, q);
            assert!(
                (a.x + b.x).abs() < 1e-6 && (a.y - b.y).abs() < 1e-6,
                "{a} vs {b}"
            );
            assert!((da - db).abs() < 1e-6);
        }
        // A point above the plane shows where its mirror image under the plane would be seen.
        let top = DVec3::new(-3.0, 6.0, -8.0);
        let (a, _) = view(DVec3::new(top.x, 2.0 - (top.y - 2.0), top.z));
        let (b, _) = seen(&p, top);
        assert!(
            (a.x + b.x).abs() < 1e-6 && (a.y - b.y).abs() < 1e-6,
            "{a} vs {b}"
        );
        // A proper rotation: winding and culling stay as they are.
        let r = p.transform.rotation();
        assert!((r.length() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn the_near_plane_clips_only_what_lies_under_the_plane() {
        let mirror = PlanarReflection::new(Vec2::new(60.0, 60.0));
        let at = GlobalTransform::from_translation_rotation(DVec3::ZERO, Quat::IDENTITY);
        let eye = looking(DVec3::new(0.0, 2.0, 10.0), DVec3::new(0.0, 0.0, -10.0));
        let p = place(
            &mirror,
            &at,
            &Camera::perspective(60.0),
            &eye,
            UVec2::new(800, 600),
        )
        .expect("seen");
        assert!(p.near > 1.0, "near {}", p.near);
        // Everything in the frustum closer than the near plane is under the water: sample
        // rays across the frustum and walk them up to the near plane.
        let rot = p.transform.rotation().as_dquat();
        let o = p.transform.translation();
        let t = p.tangents;
        for i in 0..=8 {
            for j in 0..=8 {
                let x = t.x + (t.y - t.x) * i as f32 / 8.0;
                let y = t.z + (t.w - t.z) * j as f32 / 8.0;
                let q = o + rot * DVec3::new(x as f64, y as f64, -1.0) * p.near as f64;
                assert!(q.y <= 1e-6, "({x}, {y}) reaches {q} before the near plane");
            }
        }
        // The picture covers only the screen's part below the horizon (the lake), and a
        // margin: its top edge is about level with the camera's view.
        let margin = 2.0 * MARGIN * (30.0f32).to_radians().tan();
        assert!(p.tangents.w < 0.2 + margin, "{}", p.tangents);
    }

    #[test]
    fn no_view_from_below_or_when_the_mirror_is_off_screen() {
        let mirror = PlanarReflection::new(Vec2::new(10.0, 10.0));
        let at =
            GlobalTransform::from_translation_rotation(DVec3::new(0.0, 1.0, 0.0), Quat::IDENTITY);
        let camera = Camera::perspective(60.0);
        let below = looking(DVec3::new(0.0, 0.5, 8.0), DVec3::new(0.0, 1.0, 0.0));
        assert!(place(&mirror, &at, &camera, &below, UVec2::new(800, 600)).is_none());
        let away = looking(DVec3::new(0.0, 3.0, 8.0), DVec3::new(0.0, 3.0, 30.0));
        assert!(place(&mirror, &at, &camera, &away, UVec2::new(800, 600)).is_none());
    }

    #[test]
    fn a_small_mirror_gets_a_small_picture() {
        let at = GlobalTransform::from_translation_rotation(DVec3::ZERO, Quat::IDENTITY);
        let eye = looking(DVec3::new(0.0, 6.0, 6.0), DVec3::ZERO);
        let camera = Camera::perspective(60.0);
        let viewport = UVec2::new(800, 600);
        let small = place(
            &PlanarReflection::new(Vec2::splat(1.0)),
            &at,
            &camera,
            &eye,
            viewport,
        )
        .expect("seen");
        let big = place(
            &PlanarReflection::new(Vec2::splat(100.0)),
            &at,
            &camera,
            &eye,
            viewport,
        )
        .expect("seen");
        // The mirror itself, without the margin around it on each side.
        let mirror = small.pixels - viewport.as_vec2() * 2.0 * MARGIN;
        assert!(mirror.x < 100.0 && mirror.y < 100.0, "{}", small.pixels);
        assert!(
            (big.pixels - viewport.as_vec2() * (1.0 + 2.0 * MARGIN))
                .abs()
                .max_element()
                < 1.0,
            "{}",
            big.pixels
        );
        assert_eq!(picture_mips(UVec2::new(400, 300)), 6);
        assert_eq!(picture_mips(UVec2::new(64, 32)), 3);
    }
}
