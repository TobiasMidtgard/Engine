//! Portals: a rectangle that shows another place, as if the two places were joined there
//! (Portal, Prey, Splitgate), seamlessly: through it the camera sees what it would see standing
//! beyond the exit.
//!
//! Each [`Portal`] the main camera sees gets an extra view ([`crate::ExtraView`]) whose camera
//! the renderer places every frame, just before the extra views render
//! ([`place_portal_views`]): the main camera's eye carried through the portal, facing the
//! exit's plane, with an off-axis frustum through the portal's corners (Kooima's generalized
//! perspective projection) whose near plane lies in the exit's plane, so nothing behind the exit
//! shows and the projection stays an ordinary reverse-Z one (no oblique clip plane).
//!
//! [`PortalFeature`] draws the view's HDR scene color (after its anti-aliasing, before bloom and
//! tonemapping) on the portal's rectangle in the main view, rescaled to the main view's
//! exposure and without fog (the view's own fog covers the whole way), and writes the motion of
//! what lies beyond the portal into the prepass (from the view's depth). So the main view's
//! exposure, bloom, tonemapping and anti-aliasing treat it as the rest of the scene.
//! See `engine-design/portals.md`.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use engine_core::prelude::*;

use crate::components::{Camera, ExtraView, Projection};
use crate::features::{FrameContext, GpuShared, RenderFeature, RenderPhase};
use crate::frame::{RenderingExtraView, ViewData, perspective_infinite_reverse};
use crate::settings::RenderSettings;

/// A rectangle `size` meters large in this entity's XY plane, seen from its +Z side, that shows
/// what lies beyond `exit`: the view comes out of the exit's +Z side as if the camera had walked
/// through (a half turn about the exit's up axis). Two portals that are each other's exit make a
/// two-way door. Scale on this entity's transform scales the rectangle.
///
/// Costs an extra view of the scene while the main camera sees it from the front, at about the
/// pixels it covers on screen (times `resolution`). Carry things through with [`Portal::carry`].
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[require(Transform)]
#[serde(default)]
pub struct Portal {
    /// Where it leads: an entity with a transform (often another portal). Shows nothing without.
    pub exit: Option<Entity>,
    /// Width and height, meters.
    pub size: Vec2,
    /// The picture's resolution against the pixels the portal covers on screen (0.05..2).
    pub resolution: f32,
    /// Quality of the view through it instead of the game's (`Scalability`).
    pub quality: Option<engine_world::Scalability>,
}

impl Default for Portal {
    fn default() -> Self {
        Self {
            exit: None,
            size: Vec2::new(1.2, 2.2),
            resolution: 1.0,
            quality: None,
        }
    }
}

impl Portal {
    pub fn new(exit: Entity, size: Vec2) -> Self {
        Self {
            exit: Some(exit),
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

    /// The map from in front of the portal at `entry` to beyond `exit`: apply it to a position,
    /// direction or transform that goes through (the player, a body, a projectile) to get where
    /// it comes out. Scale is ignored.
    pub fn carry(entry: &GlobalTransform, exit: &GlobalTransform) -> DAffine3 {
        rigid(exit) * DAffine3::from_rotation_y(std::f64::consts::PI) * rigid(entry).inverse()
    }
}

fn rigid(t: &GlobalTransform) -> DAffine3 {
    DAffine3::from_rotation_translation(t.rotation().as_dquat(), t.translation())
}

/// On a portal's view camera (spawned by [`place_portal_views`]): its off-axis frustum, which
/// `build_view` uses instead of the camera's projection, and its picture's size.
#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct PortalCamera {
    pub portal: Entity,
    /// Left, right, bottom, top edges of the frustum as tangents (x / -z, y / -z).
    pub tangents: Vec4,
    /// Distance to the exit's plane, meters.
    pub near: f32,
    /// Picture size, pixels.
    pub size: UVec2,
}

/// The view camera of each portal.
#[derive(Resource, Default)]
pub(crate) struct PortalViews(HashMap<Entity, Entity>);

/// What each portal's view rendered last: its HDR scene color and depth with the frustum and
/// exposure they were made with.
#[derive(Resource, Default)]
pub(crate) struct PortalPictures(pub HashMap<Entity, PortalPicture>);

pub(crate) struct PortalPicture {
    /// The view's camera.
    pub camera: Entity,
    pub color: wgpu::TextureView,
    pub depth: wgpu::TextureView,
    pub tangents: Vec4,
    pub near: f32,
    /// The view's pre-exposure.
    pub exposure: f32,
}

/// Where a portal's view camera goes this frame.
struct Placement {
    transform: GlobalTransform,
    tangents: Vec4,
    near: f32,
    /// The portal's longest edges on screen, pixels.
    pixels: Vec2,
}

/// Where the view through `portal` (at `entry`, leading to `exit`) is seen from for a camera
/// `camera` at `eye` with a `viewport`-sized picture: `None` when the camera is behind the
/// portal or doesn't see it.
fn place(
    portal: &Portal,
    entry: &GlobalTransform,
    exit: &GlobalTransform,
    camera: &Camera,
    eye: &GlobalTransform,
    viewport: UVec2,
) -> Option<Placement> {
    let Projection::Perspective { fov_y_deg, near } = camera.projection else {
        return None;
    };
    let scale = entry.scale();
    let half = (portal.size * Vec2::new(scale.x, scale.y)).abs() * 0.5;
    if half.min_element() <= 1e-4 {
        return None;
    }
    let entry_rot = entry.rotation().as_dquat();
    // The eye in the portal's frame: in front of it at `local.z`.
    let local = entry_rot.inverse() * (eye.translation() - entry.translation());
    if local.z <= 1e-3 {
        return None;
    }
    let viewport = viewport.max(UVec2::ONE).as_vec2();
    let view = Mat4::from_quat(eye.rotation().inverse());
    let fov = fov_y_deg.clamp(1.0, 170.0).to_radians();
    let view_proj =
        perspective_infinite_reverse(fov, viewport.x / viewport.y, near.max(1e-4)) * view;
    let center = (entry.translation() - eye.translation()).as_vec3();
    if !Frustum::from_view_proj(&view_proj).intersects_sphere(center, half.length()) {
        return None;
    }
    // Corners: (-x, -y), (+x, -y), (+x, +y), (-x, +y).
    let corner = |sx: f64, sy: f64| {
        let p = entry_rot * DVec3::new(sx * half.x as f64, sy * half.y as f64, 0.0);
        let clip = view_proj * (center + p.as_vec3()).extend(1.0);
        (clip.w > 1e-4).then(|| Vec2::new(clip.x, clip.y) / clip.w * 0.5 * viewport)
    };
    let pixels = match [
        corner(-1.0, -1.0),
        corner(1.0, -1.0),
        corner(1.0, 1.0),
        corner(-1.0, 1.0),
    ] {
        [Some(a), Some(b), Some(c), Some(d)] => Vec2::new(
            a.distance(b).max(d.distance(c)),
            a.distance(d).max(b.distance(c)),
        )
        .min(viewport * 2.0),
        // A corner behind the camera: the portal fills the view.
        _ => viewport,
    };
    let d = local.z;
    let tangents = Vec4::new(
        ((-(half.x as f64) - local.x) / d) as f32,
        ((half.x as f64 - local.x) / d) as f32,
        ((-(half.y as f64) - local.y) / d) as f32,
        ((half.y as f64 - local.y) / d) as f32,
    );
    // The eye carried through, facing the exit's plane (the portal's frame turned half a
    // circle onto the exit).
    let turn = DQuat::from_rotation_y(std::f64::consts::PI);
    let exit_rot = exit.rotation().as_dquat() * turn;
    Some(Placement {
        transform: GlobalTransform::from_translation_rotation(
            exit.translation() + exit_rot * local,
            exit_rot.as_quat(),
        ),
        tangents,
        near: d as f32,
        pixels,
    })
}

/// A picture size for `pixels` on screen at `resolution`: whole steps of 32, kept while the
/// previous size is enough and not much more, so it doesn't reallocate every frame.
fn picture_size(pixels: Vec2, resolution: f32, previous: UVec2) -> UVec2 {
    let want = pixels * resolution.clamp(0.05, 2.0);
    let axis = |want: f32, previous: u32| {
        let step = ((want / 32.0).ceil() as u32 * 32).clamp(32, 4096);
        let fits = previous >= step && (previous as f32) <= want * 1.5 + 32.0;
        if fits { previous } else { step }
    };
    UVec2::new(axis(want.x, previous.x), axis(want.y, previous.y))
}

/// Place every portal's view camera for the main camera: spawn it for a new portal, place it
/// and size its picture, turn it off while the main camera is behind the portal or doesn't see
/// it, and drop it (and its picture) with its portal. The renderer calls this before the extra
/// views render.
pub(crate) fn place_portal_views(world: &mut World) {
    let portals: Vec<(Entity, Portal, GlobalTransform)> = world
        .query::<(Entity, &Portal, &GlobalTransform)>()
        .iter(world)
        .map(|(e, p, gt)| (e, *p, *gt))
        .collect();
    if portals.is_empty() && !world.contains_resource::<PortalViews>() {
        return;
    }
    let mut views = world.remove_resource::<PortalViews>().unwrap_or_default();
    let gone: Vec<(Entity, Entity)> = views
        .0
        .iter()
        .filter(|(portal, _)| !portals.iter().any(|p| p.0 == **portal))
        .map(|(p, c)| (*p, *c))
        .collect();
    for (portal, camera) in gone {
        views.0.remove(&portal);
        world.despawn(camera);
        if let Some(mut pictures) = world.get_resource_mut::<PortalPictures>() {
            pictures.0.remove(&portal);
        }
    }
    let main = crate::frame::active_camera(world);
    let window = world
        .get_resource::<Window>()
        .map(|w| UVec2::new(w.width.max(1), w.height.max(1)))
        .unwrap_or(UVec2::ONE);
    let (_, viewport) = crate::frame::camera_viewport(world, window);
    for (entity, portal, entry) in portals {
        let exit = portal
            .exit
            .and_then(|e| world.get::<GlobalTransform>(e).copied());
        let placed = match (&main, exit) {
            (Some((_, camera, eye)), Some(exit)) => {
                place(&portal, &entry, &exit, camera, eye, viewport).map(|p| (p, *camera))
            }
            _ => None,
        };
        let camera = *views.0.entry(entity).or_insert_with(|| {
            world
                .spawn((
                    Camera {
                        active: false,
                        ..Default::default()
                    },
                    ExtraView::window(),
                    PortalCamera {
                        portal: entity,
                        tangents: Vec4::new(-1.0, 1.0, -1.0, 1.0),
                        near: 1.0,
                        size: UVec2::ZERO,
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
                let previous = e.get::<PortalCamera>().map_or(UVec2::ZERO, |p| p.size);
                let size = picture_size(placement.pixels, portal.resolution, previous);
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
                    PortalCamera {
                        portal: entity,
                        tangents: placement.tangents,
                        near: placement.near,
                        size,
                    },
                ));
                if e.get::<ExtraView>()
                    .is_none_or(|x| x.quality != portal.quality)
                {
                    e.insert(ExtraView {
                        quality: portal.quality,
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
            }
        }
    }
    world.insert_resource(views);
}

/// GPU layout of `PortalUniform` in `portal.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct PortalUniform {
    corners: [[f32; 4]; 4],
    axes: [[f32; 4]; 3],
    tangents: [f32; 4],
    params: [f32; 4],
}

struct Pipelines {
    layout: wgpu::BindGroupLayout,
    opaque: wgpu::RenderPipeline,
    prepass: wgpu::RenderPipeline,
}

/// Draws each portal's picture on its rectangle (see the module docs).
#[derive(Default)]
pub struct PortalFeature {
    pipelines: Option<Pipelines>,
    buffers: HashMap<Entity, wgpu::Buffer>,
    draws: Vec<wgpu::BindGroup>,
}

impl RenderFeature for PortalFeature {
    fn name(&self) -> &str {
        "portals"
    }

    fn init(&mut self, gpu: &GpuShared) {
        let device = &gpu.device;
        let shader = gpu.shader("portal", include_str!("shaders/portal.wgsl"), true);
        let texture = |binding, sample_type| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("portal"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                texture(1, wgpu::TextureSampleType::Float { filterable: true }),
                texture(2, wgpu::TextureSampleType::Depth),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("portal"),
            bind_group_layouts: &[Some(&gpu.frame_layout), Some(&layout)],
            immediate_size: 0,
        });
        let targets_scene = [Some(wgpu::ColorTargetState {
            format: gpu.formats.hdr,
            blend: None,
            write_mask: wgpu::ColorWrites::ALL,
        })];
        let targets_prepass = gpu.prepass_targets();
        let pipeline = |label: &str, fs: &str, targets: &[Option<wgpu::ColorTargetState>]| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_portal"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: Default::default(),
                depth_stencil: Some(gpu.depth_state(true)),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets,
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        self.pipelines = Some(Pipelines {
            opaque: pipeline("portal", "fs_portal", &targets_scene),
            prepass: pipeline("portal prepass", "fs_portal_prepass", &targets_prepass),
            layout,
        });
    }

    fn prepare(
        &mut self,
        world: &mut World,
        gpu: &GpuShared,
        view: &ViewData,
        _settings: &RenderSettings,
    ) {
        self.draws.clear();
        let Some(pipelines) = &self.pipelines else {
            return;
        };
        let Some(pictures) = world.get_resource::<PortalPictures>() else {
            self.buffers.clear();
            return;
        };
        self.buffers.retain(|e, _| pictures.0.contains_key(e));
        let own = world.get_resource::<RenderingExtraView>().map(|v| v.0);
        for (&portal, picture) in &pictures.0 {
            // A view never shows its own picture (it is drawing it).
            if own == Some(picture.camera) {
                continue;
            }
            let (Some(p), Some(gt)) = (
                world.get::<Portal>(portal),
                world.get::<GlobalTransform>(portal),
            ) else {
                continue;
            };
            let scale = gt.scale();
            let half = (p.size * Vec2::new(scale.x, scale.y)).abs() * 0.5;
            let rotation = gt.rotation();
            let normal = rotation * Vec3::Z;
            let center = view.relative(gt.translation());
            // Seen from the front, and on screen.
            if normal.dot(-center) <= 0.0 || !view.frustum.intersects_sphere(center, half.length())
            {
                continue;
            }
            let corner = |sx: f32, sy: f32| {
                let c = center + rotation * Vec3::new(sx * half.x, sy * half.y, 0.0);
                [c.x, c.y, c.z, 1.0]
            };
            let axis = |v: Vec3| [v.x, v.y, v.z, 0.0];
            let uniform = PortalUniform {
                corners: [
                    corner(-1.0, -1.0),
                    corner(1.0, -1.0),
                    corner(1.0, 1.0),
                    corner(-1.0, 1.0),
                ],
                axes: [
                    axis(rotation * Vec3::X),
                    axis(rotation * Vec3::Y),
                    axis(normal),
                ],
                tangents: picture.tangents.to_array(),
                params: [
                    picture.near,
                    view.exposure / picture.exposure.max(1e-12),
                    0.0,
                    0.0,
                ],
            };
            let buffer = self.buffers.entry(portal).or_insert_with(|| {
                gpu.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("portal"),
                    size: std::mem::size_of::<PortalUniform>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            });
            gpu.queue
                .write_buffer(buffer, 0, bytemuck::bytes_of(&uniform));
            self.draws
                .push(gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("portal"),
                    layout: &pipelines.layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: buffer.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&picture.color),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::TextureView(&picture.depth),
                        },
                    ],
                }));
        }
    }

    fn draw_prepass(&self, pass: &mut wgpu::RenderPass<'_>, _ctx: &FrameContext) {
        if let Some(p) = &self.pipelines {
            self.draw_with(pass, &p.prepass);
        }
    }

    fn draw(&self, phase: RenderPhase, pass: &mut wgpu::RenderPass<'_>, _ctx: &FrameContext) {
        if phase == RenderPhase::Opaque
            && let Some(p) = &self.pipelines
        {
            self.draw_with(pass, &p.opaque);
        }
    }
}

impl PortalFeature {
    fn draw_with(&self, pass: &mut wgpu::RenderPass<'_>, pipeline: &wgpu::RenderPipeline) {
        if self.draws.is_empty() {
            return;
        }
        pass.set_pipeline(pipeline);
        for group in &self.draws {
            pass.set_bind_group(1, group, &[]);
            pass.draw(0..6, 0..1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carry_turns_what_goes_in_out_of_the_exit() {
        let entry =
            GlobalTransform::from_translation_rotation(DVec3::new(0.0, 1.0, 0.0), Quat::IDENTITY);
        let exit = GlobalTransform::from_translation_rotation(
            DVec3::new(100.0, 1.0, 50.0),
            Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
        );
        let m = Portal::carry(&entry, &exit);
        // A point 2 m in front of the entry comes out 2 m behind the exit (its −Z side).
        let p = m.transform_point3(DVec3::new(0.0, 1.0, 2.0));
        let expected = exit.translation() + exit.rotation().as_dquat() * DVec3::new(0.0, 0.0, -2.0);
        assert!(p.distance(expected) < 1e-5, "{p} vs {expected}");
        // Walking in (−Z through the entry) comes out of the exit's front (+Z).
        let d = m.transform_vector3(DVec3::NEG_Z);
        let front = exit.rotation().as_dquat() * DVec3::Z;
        assert!(d.distance(front) < 1e-5, "{d} vs {front}");
    }

    #[test]
    fn the_frustum_passes_through_the_portal_corners() {
        let portal = Portal::new(Entity::PLACEHOLDER, Vec2::new(2.0, 3.0));
        let entry =
            GlobalTransform::from_translation_rotation(DVec3::new(0.0, 1.5, 0.0), Quat::IDENTITY);
        let exit =
            GlobalTransform::from_translation_rotation(DVec3::new(50.0, 1.5, 0.0), Quat::IDENTITY);
        let eye =
            GlobalTransform::from_translation_rotation(DVec3::new(0.5, 1.7, 4.0), Quat::IDENTITY);
        let placed = place(
            &portal,
            &entry,
            &exit,
            &Camera::perspective(60.0),
            &eye,
            UVec2::new(800, 600),
        )
        .expect("seen from the front");
        assert!((placed.near - 4.0).abs() < 1e-5);
        let t = placed.tangents;
        assert!((t.x - (-1.0 - 0.5) / 4.0).abs() < 1e-6 && (t.y - (1.0 - 0.5) / 4.0).abs() < 1e-6);
        assert!((t.z - (-1.5 - 0.2) / 4.0).abs() < 1e-6 && (t.w - (1.5 - 0.2) / 4.0).abs() < 1e-6);
        // The view's eye is as far behind the exit as the camera is in front of the entry.
        let v = placed.transform.translation();
        assert!(v.distance(DVec3::new(50.0 - 0.5, 1.7, -4.0)) < 1e-9, "{v}");
        // Behind the portal: no view.
        let behind =
            GlobalTransform::from_translation_rotation(DVec3::new(0.0, 1.5, -3.0), Quat::IDENTITY);
        assert!(
            place(
                &portal,
                &entry,
                &exit,
                &Camera::perspective(60.0),
                &behind,
                UVec2::new(800, 600)
            )
            .is_none()
        );
    }

    #[test]
    fn picture_sizes_change_in_steps() {
        let s = picture_size(Vec2::new(300.0, 200.0), 1.0, UVec2::ZERO);
        assert_eq!(s, UVec2::new(320, 224));
        // A little smaller or larger within the step keeps the size.
        assert_eq!(picture_size(Vec2::new(290.0, 210.0), 1.0, s), s);
        // Much smaller shrinks it.
        assert_eq!(
            picture_size(Vec2::new(100.0, 100.0), 1.0, s),
            UVec2::new(128, 128)
        );
    }
}
