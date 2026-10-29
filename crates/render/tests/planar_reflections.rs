//! Planar reflections: a mirror shows what lies above its plane reflected, where the main camera
//! sees it, including what is off the top of the screen, and never what lies under the plane.
//! A small test feature draws the mirror's rectangle with its picture, as a water shader does.
//! Needs a GPU or lavapipe; passes with a note when neither is available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{
    FrameContext, GpuShared, PlanarReflection, RenderDisabled, ViewData, capture_now,
    planar_reflection_picture, rendering_reflection,
};

const WINDOW: UVec2 = UVec2::new(400, 300);
const FOV: f32 = 60.0;
const EYE: DVec3 = DVec3::new(0.0, 2.5, 5.0);
const TARGET: DVec3 = DVec3::new(0.0, 0.0, -2.0);
/// The mirror: 20 × 20 m at y = 0.
const SIZE: f32 = 20.0;
/// On screen, and its reflection too.
const RED: DVec3 = DVec3::new(0.0, 1.0, -3.0);
/// Above the top of the screen; only its reflection shows.
const YELLOW: DVec3 = DVec3::new(0.0, 6.0, -10.0);
/// Under the plane: never in the reflection (nor seen through the mirror).
const GREEN: DVec3 = DVec3::new(1.5, -1.2, -2.0);

const SHADER: &str = r#"
struct Mirror {
    corners: array<vec4<f32>, 4>,
    view_proj: mat4x4<f32>,
    params: vec4<f32>, // exposure ratio, the picture's near plane
};
@group(1) @binding(0) var<uniform> mirror: Mirror;
@group(1) @binding(1) var picture: texture_2d<f32>;
@group(1) @binding(2) var picture_depth: texture_depth_2d;

struct Out {
    @builtin(position) clip: vec4<f32>,
    @location(0) rel: vec3<f32>,
};

@vertex
fn vs_mirror(@builtin(vertex_index) index: u32) -> Out {
    var corners = array<u32, 6>(0u, 1u, 2u, 0u, 2u, 3u);
    let p = mirror.corners[corners[index]].xyz;
    var out: Out;
    out.clip = frame.view_proj * vec4<f32>(p, 1.0);
    out.rel = p;
    return out;
}

@fragment
fn fs_mirror(in: Out) -> @location(0) vec4<f32> {
    let c = mirror.view_proj * vec4<f32>(in.rel, 1.0);
    let uv = vec2<f32>(c.x / c.w * 0.5 + 0.5, 0.5 - c.y / c.w * 0.5);
    let size = vec2<f32>(textureDimensions(picture_depth));
    let texel = vec2<i32>(clamp(uv, vec2<f32>(0.0), vec2<f32>(0.999)) * size);
    let d = textureLoad(picture_depth, texel, 0);
    // What the picture holds here lies under the plane: drop it.
    if (d > 0.0 && mirror.params.y / d < c.w - 0.05) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    return vec4<f32>(textureLoad(picture, texel, 0).rgb * mirror.params.x, 1.0);
}
"#;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct MirrorUniform {
    corners: [[f32; 4]; 4],
    view_proj: [[f32; 4]; 4],
    params: [f32; 4],
}

#[derive(Default)]
struct MirrorFeature {
    pipeline: Option<(wgpu::RenderPipeline, wgpu::BindGroupLayout, wgpu::Buffer)>,
    group: Option<wgpu::BindGroup>,
}

impl RenderFeature for MirrorFeature {
    fn name(&self) -> &str {
        "test mirror"
    }

    fn init(&mut self, gpu: &GpuShared) {
        let device = &gpu.device;
        let shader = gpu.shader("test mirror", SHADER, true);
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
            label: Some("test mirror"),
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
            label: Some("test mirror"),
            bind_group_layouts: &[Some(&gpu.frame_layout), Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("test mirror"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_mirror"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: Some(gpu.depth_state(true)),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_mirror"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: gpu.formats.hdr,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("test mirror"),
            size: std::mem::size_of::<MirrorUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.pipeline = Some((pipeline, layout, buffer));
    }

    fn prepare(
        &mut self,
        world: &mut World,
        gpu: &GpuShared,
        view: &ViewData,
        _settings: &RenderSettings,
    ) {
        self.group = None;
        let Some((_, layout, buffer)) = &self.pipeline else {
            return;
        };
        let mirror = world
            .query_filtered::<Entity, With<PlanarReflection>>()
            .iter(world)
            .next();
        let Some(mirror) = mirror else {
            return;
        };
        // The mirror's own view doesn't draw the mirror.
        if rendering_reflection(world) == Some(mirror) {
            return;
        }
        let Some(picture) = planar_reflection_picture(world, mirror) else {
            return;
        };
        let h = SIZE * 0.5;
        let corner = |x: f32, z: f32| {
            let c = view.relative(DVec3::new(x as f64, 0.0, z as f64));
            [c.x, c.y, c.z, 1.0]
        };
        let uniform = MirrorUniform {
            corners: [corner(-h, -h), corner(h, -h), corner(h, h), corner(-h, h)],
            view_proj: picture.view_proj_from(view.origin).to_cols_array_2d(),
            params: [
                view.exposure / picture.exposure.max(1e-12),
                picture.near,
                0.0,
                0.0,
            ],
        };
        gpu.queue
            .write_buffer(buffer, 0, bytemuck::bytes_of(&uniform));
        self.group = Some(gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("test mirror"),
            layout,
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

    fn draw(&self, phase: RenderPhase, pass: &mut wgpu::RenderPass<'_>, _ctx: &FrameContext) {
        if phase != RenderPhase::Opaque {
            return;
        }
        if let (Some((pipeline, ..)), Some(group)) = (&self.pipeline, &self.group) {
            pass.set_pipeline(pipeline);
            pass.set_bind_group(1, group, &[]);
            pass.draw(0..6, 0..1);
        }
    }
}

fn ball(w: &mut World, at: DVec3, radius: f32, color: (f32, f32, f32)) {
    w.spawn((
        Transform::from_translation(at),
        MeshRenderer::new(
            Shape::Sphere {
                radius,
                segments: 48,
            },
            StandardMaterial::color(color.0, color.1, color.2),
        ),
    ));
}

/// Where the main camera sees `p`, pixels.
fn pixel(p: DVec3) -> IVec2 {
    let view = Transform::from_translation(EYE).looking_at(TARGET, Vec3::Y);
    let v = view.rotation.inverse() * (p - EYE).as_vec3();
    let aspect = WINDOW.x as f32 / WINDOW.y as f32;
    let f = 1.0 / (FOV.to_radians() * 0.5).tan();
    let ndc = Vec2::new(f / aspect * v.x, f * v.y) / -v.z;
    IVec2::new(
        ((ndc.x + 1.0) * 0.5 * WINDOW.x as f32).floor() as i32,
        ((1.0 - ndc.y) * 0.5 * WINDOW.y as f32).floor() as i32,
    )
}

fn mirrored(p: DVec3) -> DVec3 {
    DVec3::new(p.x, -p.y, p.z)
}

fn render(name: &str) -> Option<(image::RgbImage, f64)> {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = WINDOW.x;
        w.height = WINDOW.y;
    }
    app.add_plugins(RenderPlugin);
    app.add_render_feature(MirrorFeature::default());
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        s.anti_aliasing = AntiAliasing::None;
        s.film_grain = 0.0;
        s.screen_space_gi = false;
        s.probe_gi = false;
    }
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.0 },
            ..Camera::perspective(FOV)
        },
        Transform::from_translation(EYE).looking_at(TARGET, Vec3::Y),
    ));
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.4, -0.8, -0.45), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        PlanarReflection::new(Vec2::splat(SIZE)).with_resolution(1.0),
    ));
    ball(w, RED, 0.6, (0.85, 0.15, 0.1));
    ball(w, YELLOW, 1.0, (0.9, 0.8, 0.1));
    ball(w, GREEN, 0.7, (0.1, 0.9, 0.15));
    // A shallow bed just under the mirror, as a lake's: beyond the picture's near plane, so
    // only the clip plane keeps it from hiding everything the mirror shows.
    w.spawn((
        Transform::from_translation(DVec3::new(0.0, -0.15, -2.0)),
        MeshRenderer::new(
            Shape::Cuboid {
                size: [SIZE, 0.1, SIZE],
            },
            StandardMaterial::color(0.1, 0.2, 0.8),
        ),
    ));
    app.update_n(5);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let dir = std::env::temp_dir().join("engine-planar-reflections");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(name);
    capture_now(&mut app.world, &path).expect("screenshot");
    if let Ok(out) = std::env::var("ENGINE_SHOTS") {
        let _ = std::fs::copy(&path, std::path::Path::new(&out).join(name));
    }
    let views = app
        .world
        .resource::<Diagnostics>()
        .get("render.extra_views")
        .unwrap_or(0.0);
    Some((image::open(&path).expect("read the shot").to_rgb8(), views))
}

#[test]
fn a_mirror_reflects_what_is_above_it_even_off_screen_and_nothing_under_it() {
    let Some((shot, views)) = render("mirror.png") else {
        return;
    };
    assert_eq!(views, 1.0, "render.extra_views");
    let at = |p: DVec3| {
        let q = pixel(p);
        let inside = q.cmpge(IVec2::ZERO).all() && q.cmplt(WINDOW.as_ivec2()).all();
        assert!(inside, "{p} is off screen at {q}");
        shot.get_pixel(q.x as u32, q.y as u32).0.map(i32::from)
    };
    // The yellow ball is above the top of the screen: the camera doesn't see it.
    assert!(
        pixel(YELLOW).y < 0,
        "yellow is on screen at {}",
        pixel(YELLOW)
    );
    let red = at(mirrored(RED));
    assert!(
        red[0] > red[1] + 60 && red[0] > red[2] + 60,
        "the red ball's reflection: {red:?}"
    );
    let yellow = at(mirrored(YELLOW));
    assert!(
        yellow[0] > yellow[2] + 60 && yellow[1] > yellow[2] + 60,
        "the yellow ball's reflection: {yellow:?}"
    );
    // The red ball itself, above the mirror, is red too.
    let direct = at(RED);
    assert!(direct[0] > direct[2] + 60, "the red ball: {direct:?}");
    let green = shot
        .pixels()
        .filter(|p| p.0[1] as i32 > p.0[0] as i32 + 60 && p.0[1] as i32 > p.0[2] as i32 + 60)
        .count();
    assert!(
        green == 0,
        "{green} pixels show the green ball under the mirror"
    );
}
