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
//!
//! With `trial_cut_more::RAY_MIRROR_PICTURES` the frame's rays trace the picture instead
//! ([`crate::ray_mirror`]): the view camera is placed as before ([`TracedPlacement`]) but not
//! drawn, and each texel's ray from the mirrored eye goes on from where it meets the plane as
//! the main view's ray reflected there. The picture has the same mip chain, depth, projection
//! ([`mirror_view_proj`]) and exposure, so its readers sample it as before. With
//! `trial_cut_more::RAY_MIRROR_FAR` the view is drawn as the picture's far view: through the
//! same frustum, at `RenderSettings::ray_mirror_far_scale` of its size, with its near plane a
//! tenth short of where the trace's shortest rays end ([`far_near`]), so it draws only what
//! lies past them and a little more; the trace takes its colour past its rays' reach (its sky
//! too, where the screen doesn't show the sky there) and blends its last tenth into it
//! ([`MirrorPictures::far`]).

use std::collections::{HashMap, HashSet};

use engine_core::prelude::*;

use crate::components::{Camera, ExtraView, Projection};
use crate::frame::{RenderingExtraView, perspective_infinite_reverse};

/// A flat mirror in this entity's XZ plane (its local +Y is the side it reflects), `size`
/// meters across (X by Z, scaled by the transform), centered on the entity.
///
/// Costs an extra view of the scene while the main camera sees it from above, at about the
/// pixels it covers on screen times `resolution` (none while the frame's rays trace its picture,
/// `trial_cut_more::RAY_MIRROR_PICTURES`; a small one of what lies past them with
/// `trial_cut_more::RAY_MIRROR_FAR`).
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
    /// Off: no view or picture, as while the camera doesn't see it, but the view's state (its
    /// targets) is kept, so turning it on again doesn't build it anew (a mirror that moves
    /// from lake to lake is switched off between them rather than despawned).
    pub on: bool,
}

impl Default for PlanarReflection {
    fn default() -> Self {
        Self {
            size: Vec2::splat(10.0),
            resolution: 0.5,
            quality: None,
            on: true,
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
    /// Near plane, meters: the nearest the plane comes inside the frustum (the far view's: a
    /// little short of where the trace's shortest rays begin to blend into it, a tenth of the
    /// reach short of their end, [`far_near`]).
    pub near: f32,
    /// The view's size, pixels: the picture's, or the far view's.
    pub size: UVec2,
    /// The picture's size, pixels, drawn or traced (what the next frame's size keeps to).
    pub picture: UVec2,
    /// The view is a traced picture's far view (`trial_cut_more::RAY_MIRROR_FAR`): what it
    /// draws goes to [`MirrorPictures::far`], not to the mirror's picture.
    pub far: bool,
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

/// The view camera of each mirror, and which mirrors' pictures the frame's rays trace.
#[derive(Resource, Default)]
pub(crate) struct MirrorViews {
    cameras: HashMap<Entity, Entity>,
    /// The mirrors whose picture (in [`MirrorPictures`]) the frame's rays traced
    /// ([`keep_traced`]).
    traced: HashSet<Entity>,
    /// Mirrors drawn again this frame after a traced picture: the view's first picture is not
    /// kept ([`keep_picture`]), so that a reader sees none for a frame and starts over, as when
    /// a mirror comes on (the water lets a mirror's first pictures settle before it shows them:
    /// a view that has not drawn for a while starts without history or metered exposure).
    resettle: HashSet<Entity>,
}

/// A mirror whose picture the frame's rays trace this frame ([`place_reflection_views`] with
/// `traced`): its view camera, placed as for drawing but off, and that view.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TracedPlacement {
    pub mirror: Entity,
    pub camera: Entity,
    pub view: MirrorCamera,
    /// The view camera's transform: the mirrored eye, turned as a proper camera.
    pub at: GlobalTransform,
}

/// The unjittered view-projection of mirror view `m` from a camera at `at` (relative to its
/// eye): what `build_view` makes of the view camera, and so a picture's `view_proj`.
pub(crate) fn mirror_view_proj(m: &MirrorCamera, at: &GlobalTransform) -> Mat4 {
    crate::frame::off_axis_infinite_reverse(m.tangents, m.near)
        * Mat4::from_quat(at.rotation().inverse())
}

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
    /// The far views drawn this frame for traced pictures (`trial_cut_more::RAY_MIRROR_FAR`),
    /// which the frame's ray pass reads past a texel's ray's reach (laid by its coverage over
    /// what the screen shows along the ray, or the sky: the screen's, else the view's own clear
    /// of the cone round the light, else the environment map's; the screen isn't marched where
    /// the view covers the whole read), and where a hit lies in the reach's last
    /// [`crate::ray_mirror::MIRROR_FAR_OVERLAP`], which blends into it by distance.
    /// Emptied as the views are placed, before they draw.
    pub far: HashMap<Entity, PlanarReflectionPicture>,
}

/// The far view's near plane for a traced picture through `tangents` (as [`MirrorCamera`]'s)
/// whose nearest point of the plane is `near` away (m): where the trace's shortest rays begin
/// to blend into it. A texel's ray from the mirrored eye, 1 along the view's axis, is
/// `sqrt(1 + x² + y²)` long for a unit of depth, and meets the plane no nearer than `near`; the
/// trace's ray goes on from there [`crate::ray_mirror::MIRROR_RAY_M`] (a texel at the frustum's
/// corner the least deep), its hits blending into the far view over the last
/// [`crate::ray_mirror::MIRROR_FAR_OVERLAP`] of that. A little short of where that begins, so
/// that every hit that blends lies in the view.
pub(crate) fn far_near(tangents: Vec4, near: f32) -> f32 {
    use crate::ray_mirror::{MIRROR_FAR_OVERLAP, MIRROR_RAY_M};
    let x = tangents.x.abs().max(tangents.y.abs());
    let y = tangents.z.abs().max(tangents.w.abs());
    let len = (1.0 + x * x + y * y).sqrt();
    (near + MIRROR_RAY_M * (1.0 - MIRROR_FAR_OVERLAP) / len) * 0.98
}

/// The far view's size for a `picture`-sized traced picture at `scale` each way.
fn far_size(picture: UVec2, scale: f32) -> UVec2 {
    (picture.as_vec2() * scale.clamp(0.05, 1.0))
        .ceil()
        .as_uvec2()
        .max(UVec2::splat(8))
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
/// the extra views render. `traced`: the frame's rays trace the pictures
/// (`trial_cut_more::RAY_MIRROR_PICTURES`); each placed camera is then left off, and the
/// placements returned for the renderer to trace ([`crate::ray_mirror`]). `far`: with
/// `traced`, each placed camera draws its picture's far view at this scale of its size
/// (`trial_cut_more::RAY_MIRROR_FAR`, `RenderSettings::ray_mirror_far_scale`).
pub(crate) fn place_reflection_views(
    world: &mut World,
    traced: bool,
    far: Option<f32>,
) -> Vec<TracedPlacement> {
    let far = far.filter(|_| traced);
    if let Some(mut pictures) = world.get_resource_mut::<MirrorPictures>() {
        pictures.far.clear();
    }
    let mirrors: Vec<(Entity, PlanarReflection, GlobalTransform)> = world
        .query::<(Entity, &PlanarReflection, &GlobalTransform)>()
        .iter(world)
        .map(|(e, m, gt)| (e, *m, *gt))
        .collect();
    if mirrors.is_empty() && !world.contains_resource::<MirrorViews>() {
        return Vec::new();
    }
    let mut views = world.remove_resource::<MirrorViews>().unwrap_or_default();
    // (What `keep_picture` did not take last frame: its view did not draw.)
    views.resettle.clear();
    let gone: Vec<(Entity, Entity)> = views
        .cameras
        .iter()
        .filter(|(mirror, _)| !mirrors.iter().any(|m| m.0 == **mirror))
        .map(|(m, c)| (*m, *c))
        .collect();
    for (mirror, camera) in gone {
        views.cameras.remove(&mirror);
        views.traced.remove(&mirror);
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
    let mut placed = Vec::new();
    for (entity, mirror, at) in mirrors {
        let placement = main
            .as_ref()
            .filter(|_| mirror.on)
            .and_then(|(_, camera, eye)| {
                place(&mirror, &at, camera, eye, viewport).map(|p| (p, *camera))
            });
        let camera = *views.cameras.entry(entity).or_insert_with(|| {
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
                        picture: UVec2::ZERO,
                        far: false,
                        plane_point: at.translation(),
                        plane_normal: at.rotation() * Vec3::Y,
                    },
                ))
                .id()
        });
        let Ok(mut e) = world.get_entity_mut(camera) else {
            views.cameras.remove(&entity);
            views.traced.remove(&entity);
            continue;
        };
        match placement {
            Some((placement, main_camera)) => {
                let previous = e.get::<MirrorCamera>().map_or(UVec2::ZERO, |m| m.picture);
                let size =
                    crate::portal::picture_size(placement.pixels, mirror.resolution, previous);
                let view = MirrorCamera {
                    mirror: entity,
                    tangents: placement.tangents,
                    near: placement.near,
                    size,
                    picture: size,
                    far: false,
                    plane_point: at.translation(),
                    plane_normal: (at.rotation() * Vec3::Y).normalize(),
                };
                // What the camera draws: the picture, nothing while it is traced, or the
                // traced picture's far view.
                let drawn = match far {
                    Some(scale) => MirrorCamera {
                        near: far_near(placement.tangents, placement.near),
                        size: far_size(size, scale),
                        far: true,
                        ..view
                    },
                    None => view,
                };
                e.insert((
                    Camera {
                        projection: Projection::Perspective {
                            fov_y_deg: 60.0,
                            near: drawn.near,
                        },
                        exposure: main_camera.exposure,
                        active: !traced || far.is_some(),
                        order: main_camera.order.saturating_sub(1),
                    },
                    placement.transform.to_transform(),
                    placement.transform,
                    drawn,
                ));
                if e.get::<ExtraView>()
                    .is_none_or(|x| x.quality != mirror.quality)
                {
                    e.insert(ExtraView {
                        quality: mirror.quality,
                        ..ExtraView::window()
                    });
                }
                if traced {
                    placed.push(TracedPlacement {
                        mirror: entity,
                        camera,
                        view,
                        at: placement.transform,
                    });
                } else if views.traced.remove(&entity) {
                    // Drawn again: the traced picture goes, and the view's first is not kept.
                    if let Some(mut pictures) = world.get_resource_mut::<MirrorPictures>() {
                        pictures.pictures.remove(&entity);
                    }
                    views.resettle.insert(entity);
                }
            }
            None => {
                if let Some(mut c) = e.get_mut::<Camera>()
                    && c.active
                {
                    c.active = false;
                }
                views.traced.remove(&entity);
                if let Some(mut pictures) = world.get_resource_mut::<MirrorPictures>() {
                    pictures.pictures.remove(&entity);
                }
            }
        }
    }
    world.insert_resource(views);
    placed
}

/// Keep `picture`, which the frame's rays trace this frame, as `mirror`'s
/// ([`crate::ray_mirror`]).
pub(crate) fn keep_traced(world: &mut World, mirror: Entity, picture: PlanarReflectionPicture) {
    world
        .get_resource_or_init::<MirrorPictures>()
        .pictures
        .insert(mirror, picture);
    world
        .get_resource_or_init::<MirrorViews>()
        .traced
        .insert(mirror);
}

/// `mirror` was placed to be traced, but the frame's rays trace nothing this frame (the ray
/// scene went): no picture, and its readers start over.
pub(crate) fn drop_traced(world: &mut World, mirror: Entity) {
    if let Some(mut pictures) = world.get_resource_mut::<MirrorPictures>() {
        pictures.pictures.remove(&mirror);
    }
    if let Some(mut views) = world.get_resource_mut::<MirrorViews>() {
        views.traced.remove(&mirror);
    }
}

/// Mip levels of a picture `size` pixels: down to about 8 pixels on its short side, at most 6.
pub(crate) fn picture_mips(size: UVec2) -> u32 {
    let short = size.min_element().max(1);
    (32 - (short / 8).max(1).leading_zeros()).clamp(1, 6)
}

/// A mirror view's mip chain for its picture of the view's HDR scene colour `hdr`, made or
/// remade at its size: the view's own commands copy the colour into it and filter the levels
/// ([`record_picture_levels`], so that their GPU time is the view's), then [`keep_picture`]
/// keeps the picture.
pub(crate) fn mirror_chain(
    world: &mut World,
    gpu: &crate::features::GpuShared,
    mirror: &MirrorCamera,
    hdr: &wgpu::Texture,
) -> wgpu::Texture {
    let size = UVec2::new(hdr.width(), hdr.height());
    let mips = picture_mips(size);
    let format = hdr.format();
    let mut pictures = world.get_resource_or_init::<MirrorPictures>();
    let chain = pictures
        .chains
        .entry(mirror.mirror)
        .or_insert_with(|| make_chain(gpu, size, mips, format));
    if chain.width() != size.x || chain.height() != size.y || chain.mip_level_count() != mips {
        *chain = make_chain(gpu, size, mips, format);
    }
    chain.clone()
}

/// Copy a mirror view's HDR scene colour `hdr` into its chain ([`mirror_chain`]) and filter
/// the levels, at the end of the view's own commands.
pub(crate) fn record_picture_levels(
    gpu: &crate::features::GpuShared,
    encoder: &mut wgpu::CommandEncoder,
    hdr: &wgpu::Texture,
    chain: &wgpu::Texture,
) {
    encoder.copy_texture_to_texture(
        hdr.as_image_copy(),
        chain.as_image_copy(),
        wgpu::Extent3d {
            width: hdr.width(),
            height: hdr.height(),
            depth_or_array_layers: 1,
        },
    );
    gpu.mips.record(gpu, encoder, chain, chain.format());
}

/// Keep a mirror view's picture, its chain filled ([`record_picture_levels`]), for features to
/// sample: the view's unjittered view-projection, eye and exposure. A far view's goes to
/// [`MirrorPictures::far`], for the trace of the mirror's picture.
pub(crate) fn keep_picture(
    world: &mut World,
    mirror: &MirrorCamera,
    camera: Entity,
    depth: &wgpu::TextureView,
    (view_proj, origin, exposure): (Mat4, DVec3, f32),
) {
    let mut pictures = world
        .remove_resource::<MirrorPictures>()
        .unwrap_or_default();
    let Some(chain) = pictures.chains.get(&mirror.mirror) else {
        world.insert_resource(pictures);
        return;
    };
    let size = UVec2::new(chain.width(), chain.height());
    let mips = chain.mip_level_count();
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
    if mirror.far {
        pictures.far.insert(mirror.mirror, picture);
        world.insert_resource(pictures);
        return;
    }
    // Drawn again after traced pictures: this first one is not kept (`MirrorViews::resettle`).
    let resettle = world
        .get_resource_mut::<MirrorViews>()
        .is_some_and(|mut views| views.resettle.remove(&mirror.mirror));
    if !resettle {
        pictures.pictures.insert(mirror.mirror, picture);
    }
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

    /// A traced picture's projection is the one its view camera would draw it with
    /// (`build_view`'s unjittered view-projection), so its readers find a plane point's
    /// reflection where they did in the drawn picture.
    #[test]
    fn a_traced_pictures_projection_is_its_views() {
        let mirror = PlanarReflection::new(Vec2::new(40.0, 40.0));
        let at =
            GlobalTransform::from_translation_rotation(DVec3::new(0.0, 2.0, 0.0), Quat::IDENTITY);
        let eye = looking(DVec3::new(3.0, 5.0, 12.0), DVec3::new(-2.0, 2.0, -6.0));
        let p = place(
            &mirror,
            &at,
            &Camera::perspective(60.0),
            &eye,
            UVec2::new(800, 450),
        )
        .expect("seen");
        let view = MirrorCamera {
            mirror: Entity::PLACEHOLDER,
            tangents: p.tangents,
            near: p.near,
            size: UVec2::new(400, 160),
            picture: UVec2::new(400, 160),
            far: false,
            plane_point: at.translation(),
            plane_normal: Vec3::Y,
        };
        let mut world = World::new();
        world.insert_resource(crate::settings::RenderSettings::default());
        world.insert_resource(crate::settings::Atmosphere::default());
        world.insert_resource(crate::components::AmbientLight::default());
        world.insert_resource(Time::default());
        let camera = world
            .spawn((
                Camera {
                    projection: Projection::Perspective {
                        fov_y_deg: 60.0,
                        near: p.near,
                    },
                    active: false,
                    ..Default::default()
                },
                p.transform,
                view,
            ))
            .id();
        world.insert_resource(RenderingExtraView(camera));
        let built = crate::frame::build_view(
            &mut world,
            400,
            160,
            None,
            Vec2::new(0.3, -0.2),
            &mut crate::frame::ExposureState::default(),
            0,
        );
        let ours = mirror_view_proj(&view, &p.transform);
        assert!(
            built.unjittered_view_proj.abs_diff_eq(ours, 1e-6),
            "{} vs {ours}",
            built.unjittered_view_proj
        );
        // And it puts a point of the plane where the main camera sees it, flipped left to
        // right (the picture's readers project plane points with it).
        let q = DVec3::new(-1.0, 2.0, 2.0);
        let rel = (q - p.transform.translation()).as_vec3();
        let c = ours * rel.extend(1.0);
        let v = eye.rotation().as_dquat().inverse() * (q - eye.translation());
        let tangent = DVec2::new(v.x, v.y) / -v.z;
        let (t, ndc) = (p.tangents, Vec2::new(c.x, c.y) / c.w);
        let expected = Vec2::new(
            ((-tangent.x as f32 - t.x) / (t.y - t.x)) * 2.0 - 1.0,
            ((tangent.y as f32 - t.z) / (t.w - t.z)) * 2.0 - 1.0,
        );
        assert!(
            (ndc - expected).abs().max_element() < 1e-4,
            "{ndc} vs {expected}"
        );
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

    /// A traced picture's far view (`trial_cut_more::RAY_MIRROR_FAR`) begins no farther than
    /// any texel's hits begin to blend into it (where its ray meets the plane, then nine tenths
    /// of the trace's reach), so every hit that blends lies in the view, and not much nearer,
    /// so that it draws little the rays hold.
    #[test]
    fn the_far_view_begins_where_the_traces_rays_blend_into_it() {
        let mirror = PlanarReflection::new(Vec2::new(400.0, 400.0));
        let at = GlobalTransform::from_translation_rotation(DVec3::ZERO, Quat::IDENTITY);
        for (eye, target) in [
            (DVec3::new(0.0, 2.0, 10.0), DVec3::new(0.0, 0.0, -10.0)),
            (DVec3::new(0.0, 25.0, 150.0), DVec3::new(0.0, 0.0, 110.0)),
            (DVec3::new(30.0, 60.0, 0.0), DVec3::new(-20.0, 0.0, -40.0)),
        ] {
            let p = place(
                &mirror,
                &at,
                &Camera::perspective(70.0),
                &looking(eye, target),
                UVec2::new(1600, 900),
            )
            .expect("seen");
            let rot = p.transform.rotation().as_dquat();
            let o = p.transform.translation();
            let n = DVec3::Y;
            let t = p.tangents;
            let near = far_near(t, p.near) as f64;
            let mut shortest = f64::INFINITY;
            for i in 0..=16 {
                for j in 0..=16 {
                    let x = t.x + (t.y - t.x) * i as f32 / 16.0;
                    let y = t.z + (t.w - t.z) * j as f32 / 16.0;
                    let d = rot * DVec3::new(x as f64, y as f64, -1.0);
                    // (From the mirrored eye under the plane, up to it.)
                    let dn = d.dot(n);
                    if dn <= 1e-9 {
                        continue;
                    }
                    // Depth (1 along the view's axis) where the ray meets the plane, then
                    // where the trace's ray from there begins to blend into the far view.
                    let u = -o.dot(n) / dn;
                    let reach = crate::ray_mirror::MIRROR_RAY_M as f64
                        * (1.0 - crate::ray_mirror::MIRROR_FAR_OVERLAP as f64);
                    let end = u + reach / d.length();
                    shortest = shortest.min(end);
                }
            }
            assert!(shortest.is_finite(), "{eye}: no texel meets the plane");
            assert!(
                near <= shortest,
                "{eye}: the far view begins at {near:.1}, a ray's blend at {shortest:.1}"
            );
            assert!(
                near >= 0.8 * shortest,
                "{eye}: the far view begins at {near:.1}, well short of {shortest:.1}"
            );
        }
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
