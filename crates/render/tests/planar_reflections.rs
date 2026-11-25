//! Planar reflections: a mirror shows what lies above its plane reflected, where the main camera
//! sees it, including what is off the top of the screen, and never what lies under the plane.
//! A small test feature draws the mirror's rectangle with its picture, as a water shader does.
//! The same with the picture traced by the frame's rays in place of the mirror's view
//! (`trial_cut_more::RAY_MIRROR_PICTURES`), which needs ray queries (lavapipe has them), its
//! hits fogged over their mirrored path as the drawn mirror's are, and a wall past the rays'
//! reach above the top of the screen in it with the picture's far view
//! (`trial_cut_more::RAY_MIRROR_FAR`) as with the mirror drawn, where the rays alone leave sky,
//! the far view adding nothing round what the rays hit and leaving no hole round it either,
//! taking over from the rays with no line along a wall across the end of their reach, and
//! giving the sky it drew where the screen doesn't show it, clouds and haze, as the drawn
//! mirror shows it; and no disc of the sun in a traced mirror, from the screen or the far view,
//! where the drawn one shows it, nor the ground round it under a low sun.
//! Needs a GPU or lavapipe; passes with a note when neither is available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::settings::shade_skip::COUNT_MESH_RUNS;
use engine_render::settings::trial_cut_more::{
    RAY_MIRROR_FAR, RAY_MIRROR_PICTURES, RAY_WORLD, RAYS,
};
use engine_render::{
    FrameContext, GpuShared, PlanarReflection, RenderDisabled, Screenshots, ViewData, capture_now,
    planar_reflection_picture, rendering_reflection,
};
use engine_world::WeatherState;

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

/// Each view [`MirrorFeature`] was asked to draw, in order: whether it was an extra view, and
/// its [`ViewData::detail`].
#[derive(Resource, Default)]
struct ViewDetails(Vec<(bool, f32)>);

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
        world
            .get_resource_or_insert_with(ViewDetails::default)
            .0
            .push((view.extra_view, view.detail));
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

/// The balls over the mirror, not yet rendered; `traced`: the frame's rays trace the mirror's
/// picture (`trial_cut_more::RAY_MIRROR_PICTURES`), with probe GI on ray-tracing hardware
/// through the frame's ray scene (a few probes a frame: their rays are most of a frame's time
/// on a software GPU), the trace counted.
fn ball_app(traced: bool) -> App {
    ball_app_with(traced, MirrorFeature::default())
}

/// [`ball_app`] with `feature` drawing the mirror's square in place of [`MirrorFeature`].
fn ball_app_with(traced: bool, feature: impl RenderFeature) -> App {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = WINDOW.x;
        w.height = WINDOW.y;
    }
    app.add_plugins(RenderPlugin);
    app.add_render_feature(feature);
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        s.anti_aliasing = AntiAliasing::None;
        s.film_grain = 0.0;
        s.screen_space_gi = false;
        s.probe_gi = false;
        // No part of the ray-traced lighting but what a test asks for, whatever the
        // defaults hold.
        s.trial_cuts_more &= !RAYS;
        if traced {
            s.probe_gi = true;
            s.gi_hardware_rays = true;
            s.gi_probe_spacing = 1.0;
            s.gi_probe_grid = [16, 8, 16];
            s.gi_probes_per_frame = 16;
            s.trial_cuts_more |= RAY_WORLD | RAY_MIRROR_PICTURES;
            s.debug_skip |= COUNT_MESH_RUNS;
        }
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
    app
}

/// What [`render`] saw: the shot and the frame's numbers.
struct Rendered {
    shot: image::RgbImage,
    diagnostics: Diagnostics,
}

impl Rendered {
    fn get(&self, key: &str) -> f64 {
        self.diagnostics.get(key).unwrap_or(-1.0)
    }
}

fn render(name: &str, traced: bool) -> Option<Rendered> {
    let mut app = ball_app(traced);
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
    Some(Rendered {
        shot: image::open(&path).expect("read the shot").to_rgb8(),
        diagnostics: app.world.resource::<Diagnostics>().clone(),
    })
}

/// The red ball's reflection, the yellow one's (the ball itself off the top of the screen), the
/// red ball itself, and no green (the ball under the plane) anywhere.
fn assert_reflects_what_is_above(shot: &image::RgbImage, what: &str) {
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
        "{what}: the red ball's reflection: {red:?}"
    );
    let yellow = at(mirrored(YELLOW));
    assert!(
        yellow[0] > yellow[2] + 60 && yellow[1] > yellow[2] + 60,
        "{what}: the yellow ball's reflection: {yellow:?}"
    );
    // The red ball itself, above the mirror, is red too.
    let direct = at(RED);
    assert!(
        direct[0] > direct[2] + 60,
        "{what}: the red ball: {direct:?}"
    );
    let green = shot
        .pixels()
        .filter(|p| p.0[1] as i32 > p.0[0] as i32 + 60 && p.0[1] as i32 > p.0[2] as i32 + 60)
        .count();
    assert!(
        green == 0,
        "{what}: {green} pixels show the green ball under the mirror"
    );
}

#[test]
fn a_mirror_reflects_what_is_above_it_even_off_screen_and_nothing_under_it() {
    let Some(r) = render("mirror.png", false) else {
        return;
    };
    assert_eq!(r.get("render.extra_views"), 1.0, "render.extra_views");
    // The picture's copy into its chain and its levels are the view's own work, timed with it
    // (where the GPU times its passes).
    if r.get("render.extra_views_gpu_ms") > 0.0 {
        assert!(
            r.get("render.extra_views_gpu.mirror picture levels") >= 0.0,
            "the picture's copy and levels are not the view's"
        );
    }
    assert_reflects_what_is_above(&r.shot, "drawn");
}

/// The same with the picture traced by the frame's rays: the mirror's view is not drawn, and
/// the picture holds the yellow ball off the top of the screen (a ray's hit, lit by its own
/// light), the red ball (the screen's, or a ray's) and nothing under the plane.
#[test]
fn a_traced_mirror_reflects_what_is_above_it_even_off_screen_and_nothing_under_it() {
    let Some(r) = render("mirror-traced.png", true) else {
        return;
    };
    if r.get("render.ray.supported") != 1.0 {
        eprintln!("skipping: the GPU cannot hold the ray scene (no ray queries)");
        return;
    }
    assert_eq!(r.get("render.ray.on"), 1.0, "the ray scene is on");
    assert_eq!(r.get("render.ray.losses"), 0.0);
    assert_eq!(r.get("render.ray.mirror_pictures"), 1.0, "a traced picture");
    // Sized to the part of the screen the mirror covers, most of the lower half of 400 x 300 at
    // resolution 1 (a picture is never under 32 x 32, 1,024 texels).
    assert!(
        r.get("render.ray.mirror_texels") > 40_000.0,
        "its texels: {}",
        r.get("render.ray.mirror_texels")
    );
    assert!(
        r.get("render.extra_views") <= 0.0,
        "no view drawn: {}",
        r.get("render.extra_views")
    );
    // Its rays were counted: the yellow ball can only be a ray's.
    assert!(
        r.get("render.ray.mirror_rays") > 0.0,
        "rays: {}",
        r.get("render.ray.mirror_rays")
    );
    assert_reflects_what_is_above(&r.shot, "traced");
}

/// A wall 1.6 km wide and 200 m high, 600 m off, past the rays' reach (384 m from the plane)
/// and above the top of the screen: only its reflection shows, across the mirror's far half.
const FAR_WALL: DVec3 = DVec3::new(0.0, 230.0, -600.0);
/// A point of its face, left of the balls, whose reflection is on the mirror.
const FAR_WALL_POINT: DVec3 = DVec3::new(-250.0, 180.0, -595.0);

/// A mountain off the top of the screen and past the rays' reach shows in a traced mirror with
/// its far view (`trial_cut_more::RAY_MIRROR_FAR`) as in the drawn one: the view is drawn again
/// at a quarter of the picture's size each way from a tenth short of where the trace's rays
/// end, and past their reach a texel takes what it drew. Without it the rays alone show the sky
/// there. What lies within the rays' reach is still theirs (the balls' reflections), and nothing
/// under the plane shows. The far view asks the features for the picture's detail
/// (`ViewData::detail`, the picture's size over its own), so the weather's clouds march as
/// finely in it as in the drawn mirror's view; every other view asks for its own.
#[test]
fn a_traced_mirrors_far_view_shows_what_lies_past_its_rays_off_the_screen() {
    let mut shots = Vec::new();
    for (traced, far, name) in [
        (false, false, "far-wall-drawn.png"),
        (true, false, "far-wall-traced.png"),
        (true, true, "far-wall-far-view.png"),
    ] {
        let mut app = ball_app(traced);
        if far {
            app.world.resource_mut::<RenderSettings>().trial_cuts_more |= RAY_MIRROR_FAR;
        }
        app.world.spawn((
            Transform::from_translation(FAR_WALL),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [1600.0, 200.0, 10.0],
                },
                StandardMaterial::color(0.75, 0.35, 0.05),
            ),
        ));
        app.update_n(5);
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("skipping: no GPU ({})", d.0);
            return;
        }
        let d = app.world.resource::<Diagnostics>().clone();
        let get = |key: &str| d.get(key).unwrap_or(-1.0);
        if traced && get("render.ray.supported") != 1.0 {
            eprintln!("skipping: the GPU cannot hold the ray scene (no ray queries)");
            return;
        }
        let details = &app.world.resource::<ViewDetails>().0;
        assert_eq!(
            details.iter().any(|&(extra, _)| extra),
            !traced || far,
            "{name}: the views drawn {details:?}"
        );
        for &(extra, detail) in details {
            let wanted = if extra && far { 4.0 } else { 1.0 };
            assert!(
                (detail - wanted).abs() < 0.1,
                "{name}: detail {detail} in a view (extra: {extra}), not {wanted}"
            );
        }
        let dir = std::env::temp_dir().join("engine-planar-reflections");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(name);
        capture_now(&mut app.world, &path).expect("screenshot");
        if let Ok(out) = std::env::var("ENGINE_SHOTS") {
            let _ = std::fs::copy(&path, std::path::Path::new(&out).join(name));
        }
        let shot = image::open(&path).expect("read the shot").to_rgb8();
        if traced {
            assert_eq!(get("render.ray.mirror_pictures"), 1.0, "{name}: traced");
            let texels = get("render.ray.mirror_texels");
            if far {
                assert_eq!(get("render.ray.mirror_far"), 1.0, "{name}: a far view");
                assert_eq!(
                    get("render.extra_views"),
                    1.0,
                    "{name}: drawn as an extra view"
                );
                let far_texels = get("render.ray.mirror_far_texels");
                assert!(
                    far_texels > 0.0 && far_texels < texels / 12.0,
                    "{name}: {far_texels} texels in the far view, {texels} in the picture"
                );
            } else {
                assert_eq!(get("render.ray.mirror_far"), 0.0, "{name}: no far view");
                assert!(get("render.extra_views") <= 0.0, "{name}: no view drawn");
            }
        }
        assert_reflects_what_is_above(&shot, name);
        shots.push(shot);
    }
    assert!(
        pixel(FAR_WALL_POINT).y < 0,
        "the wall is on screen at {}",
        pixel(FAR_WALL_POINT)
    );
    let q = pixel(mirrored(FAR_WALL_POINT));
    let at = |shot: &image::RgbImage| shot.get_pixel(q.x as u32, q.y as u32).0.map(i32::from);
    let (drawn, traced, far) = (at(&shots[0]), at(&shots[1]), at(&shots[2]));
    eprintln!("the wall's reflection at {q}: drawn {drawn:?}, traced {traced:?}, far view {far:?}");
    let orange = |c: [i32; 3]| c[0] > c[2] + 40 && c[1] > c[2];
    assert!(orange(drawn), "the drawn mirror shows the wall: {drawn:?}");
    assert!(
        !orange(traced),
        "the rays alone leave the wall's place to the sky: {traced:?}"
    );
    assert!(orange(far), "the far view shows the wall: {far:?}");
    for c in 0..3 {
        assert!(
            (far[c] - drawn[c]).abs() <= 24,
            "the far view's wall {far:?} against the drawn mirror's {drawn:?}"
        );
    }
}

/// Two pillars 20 m wide and 40 m high, 280 m off either side of the balls: within the rays'
/// reach (about 290 m along them from the plane) and past the far view's near plane (about
/// 230 m deep here), above the top of the screen, so that only their reflections show. The
/// left one stands in front of [`PILLAR_WALL`], the right one against the sky.
const PILLARS: [DVec3; 2] = [
    DVec3::new(-60.0, 90.0, -280.0),
    DVec3::new(60.0, 90.0, -280.0),
];
const PILLAR_SIZE: [f32; 3] = [20.0, 40.0, 10.0];
/// A green wall 250 m wide and 200 m high, 600 m off, past the rays' reach and above the top of
/// the screen: in the reflection it lies all round the left pillar's, which it stands behind.
const PILLAR_WALL: DVec3 = DVec3::new(-175.0, 230.0, -600.0);

/// `out` pixels out from the reflection of a pillar ([`PILLAR_SIZE`], at `pillar`), round the
/// middle half of each side: where the rays pass it.
fn pillar_ring(pillar: DVec3, out: i32) -> Vec<IVec2> {
    let [hx, hy, hz] = PILLAR_SIZE.map(|s| f64::from(s) * 0.5);
    let p = |x: f64, y: f64, z: f64| pixel(mirrored(pillar + DVec3::new(x, y, z)));
    let mut ring = Vec::new();
    for k in 0..=8 {
        let s = k as f64 / 8.0 - 0.5;
        let (y, x) = (hy * s, hx * s);
        let left = p(-hx, y, hz).x.min(p(-hx, y, -hz).x);
        let right = p(hx, y, hz).x.max(p(hx, y, -hz).x);
        let row = p(0.0, y, hz).y;
        ring.push(IVec2::new(left - out, row));
        ring.push(IVec2::new(right + out, row));
        // (The pillar's foot is the reflection's top, its head the bottom.)
        let top = p(x, -hy, hz).y.min(p(x, -hy, -hz).y);
        let bottom = p(x, hy, hz).y.max(p(x, hy, -hz).y);
        let column = p(x, 0.0, hz).x;
        ring.push(IVec2::new(column, top - out));
        ring.push(IVec2::new(column, bottom + out));
    }
    ring
}

/// What the rays hit in a traced mirror stays theirs alone with its far view
/// (`trial_cut_more::RAY_MIRROR_FAR`), which draws it too (from one near plane for the whole
/// picture, short of every texel's reach, at a quarter of the picture's size each way): beside a
/// pillar's reflection, where the rays pass it, the picture shows the sky as the drawn mirror
/// does (the one the far view drew there, which the screen doesn't show), with no quarter-size
/// fringe of the pillar round its outline, and a wall past the rays' reach behind the other
/// pillar, as the drawn mirror shows it, with no hole of sky round the pillar's outline where
/// the far view's pillar hides the wall.
#[test]
fn a_traced_mirrors_far_view_adds_no_fringe_round_what_its_rays_hit() {
    let mut shots = Vec::new();
    for (traced, far, name) in [
        (false, false, "pillars-drawn.png"),
        (true, false, "pillars-traced.png"),
        (true, true, "pillars-far-view.png"),
    ] {
        let mut app = ball_app(traced);
        if far {
            app.world.resource_mut::<RenderSettings>().trial_cuts_more |= RAY_MIRROR_FAR;
        }
        for at in PILLARS {
            app.world.spawn((
                Transform::from_translation(at),
                MeshRenderer::new(
                    Shape::Cuboid { size: PILLAR_SIZE },
                    StandardMaterial::color(0.75, 0.35, 0.05),
                ),
            ));
        }
        app.world.spawn((
            Transform::from_translation(PILLAR_WALL),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [250.0, 200.0, 10.0],
                },
                StandardMaterial::color(0.1, 0.5, 0.1),
            ),
        ));
        app.update_n(5);
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("skipping: no GPU ({})", d.0);
            return;
        }
        let d = app.world.resource::<Diagnostics>().clone();
        if traced {
            if d.get("render.ray.supported") != Some(1.0) {
                eprintln!("skipping: the GPU cannot hold the ray scene (no ray queries)");
                return;
            }
            assert_eq!(
                d.get("render.ray.mirror_pictures"),
                Some(1.0),
                "{name}: traced"
            );
            assert_eq!(
                d.get("render.ray.mirror_far"),
                Some(if far { 1.0 } else { 0.0 }),
                "{name}: its far view"
            );
        }
        let dir = std::env::temp_dir().join("engine-planar-reflections");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(name);
        capture_now(&mut app.world, &path).expect("screenshot");
        if let Ok(out) = std::env::var("ENGINE_SHOTS") {
            let _ = std::fs::copy(&path, std::path::Path::new(&out).join(name));
        }
        shots.push(image::open(&path).expect("read the shot").to_rgb8());
    }
    let (drawn, traced, far) = (&shots[0], &shots[1], &shots[2]);
    let [hx, hy, hz] = PILLAR_SIZE.map(|s| f64::from(s) * 0.5);
    let at = |shot: &image::RgbImage, q: IVec2| shot.get_pixel(q.x as u32, q.y as u32).0;
    let apart = |a: [u8; 3], b: [u8; 3]| (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap_or(0);
    let ring = pillar_ring;
    for pillar in PILLARS {
        let foot = pixel(pillar + DVec3::new(-hx, -hy, hz))
            .y
            .max(pixel(pillar + DVec3::new(hx, -hy, hz)).y);
        assert!(
            foot < 0,
            "the pillar at {pillar} is on screen down to {foot}"
        );
        // The rays hit it: its face in every picture.
        let face = pixel(mirrored(pillar + DVec3::new(0.0, 0.0, hz)));
        for (shot, name) in shots.iter().zip(["drawn", "traced", "far view"]) {
            let c = at(shot, face).map(i32::from);
            assert!(
                c[0] > c[2] + 40 && c[1] > c[2],
                "{name}: the reflection of the pillar at {pillar}, {face}: {c:?}"
            );
        }
    }
    let green = |c: [u8; 3]| c[1] > c[0].saturating_add(30) && c[1] > c[2].saturating_add(10);
    // Round the right pillar the sky, as the drawn mirror shows it (the far view's own there:
    // the screen doesn't show it), two to four pixels out (clear of the drawn and the traced
    // pictures' own outlines, which lie a little apart): a fringe of the far view's pillar would
    // tint it orange.
    let mut worst = (0, IVec2::ZERO);
    for out in 2..=4 {
        for q in ring(PILLARS[1], out) {
            let d = apart(at(drawn, q), at(far, q));
            if d > worst.0 {
                worst = (d, q);
            }
        }
    }
    eprintln!(
        "round the pillar against the sky the far view's picture is at most {} from the drawn \
         mirror's (at {})",
        worst.0, worst.1
    );
    assert!(
        worst.0 <= 8,
        "the far view fringes the pillar's reflection: {} apart from the drawn mirror at {}",
        worst.0,
        worst.1
    );
    // Round the left one the wall, as drawn, two to four pixels out (clear of the drawn and the
    // traced pictures' own outlines, which lie a little apart); with the rays alone, the sky.
    let mut worst = (0, IVec2::ZERO);
    for out in 2..=4 {
        for q in ring(PILLARS[0], out) {
            assert!(
                green(at(drawn, q)),
                "drawn: the wall at {q}: {:?}",
                at(drawn, q)
            );
            assert!(
                !green(at(traced, q)),
                "traced: the sky at {q}: {:?}",
                at(traced, q)
            );
            let d = apart(at(drawn, q), at(far, q));
            if d > worst.0 {
                worst = (d, q);
            }
        }
    }
    eprintln!(
        "round the pillar in front of the wall the far view's picture is at most {} from the \
         drawn mirror's (at {})",
        worst.0, worst.1
    );
    assert!(
        worst.0 <= 12,
        "the far view leaves a hole round the pillar's reflection in the wall's: {} apart from \
         the drawn mirror at {}",
        worst.0,
        worst.1
    );
}

/// Under temporal anti-aliasing too, as in the game, the sky beside the reflection of a pillar
/// against the sky, where the rays pass it, is the drawn mirror's: the far view's own history
/// blends about a sixth of an outline into the sky texel beside it, a quarter of the picture's
/// size across, so the picture takes the far view's sky only from texels whose eight neighbours
/// drew the sky too, and beside an outline reads it again two of the view's texels away
/// (`mirror_far_view`). Two to five pixels out, clear of the drawn mirror's own outline.
#[test]
fn a_traced_mirrors_far_view_adds_no_fringe_under_temporal_anti_aliasing() {
    let mut shots = Vec::new();
    for (traced, name) in [
        (false, "pillar-taa-drawn.png"),
        (true, "pillar-taa-far-view.png"),
    ] {
        let mut app = ball_app(traced);
        {
            let mut s = app.world.resource_mut::<RenderSettings>();
            s.anti_aliasing = AntiAliasing::Taa;
            if traced {
                s.trial_cuts_more |= RAY_MIRROR_FAR;
            }
        }
        app.world.spawn((
            Transform::from_translation(PILLARS[1]),
            MeshRenderer::new(
                Shape::Cuboid { size: PILLAR_SIZE },
                StandardMaterial::color(0.75, 0.35, 0.05),
            ),
        ));
        app.update_n(24);
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("skipping: no GPU ({})", d.0);
            return;
        }
        if traced {
            let d = app.world.resource::<Diagnostics>();
            if d.get("render.ray.supported") != Some(1.0) {
                eprintln!("skipping: the GPU cannot hold the ray scene (no ray queries)");
                return;
            }
            assert_eq!(
                d.get("render.ray.mirror_far"),
                Some(1.0),
                "{name}: its far view"
            );
        }
        let dir = std::env::temp_dir().join("engine-planar-reflections");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(name);
        capture_now(&mut app.world, &path).expect("screenshot");
        if let Ok(out) = std::env::var("ENGINE_SHOTS") {
            let _ = std::fs::copy(&path, std::path::Path::new(&out).join(name));
        }
        shots.push(image::open(&path).expect("read the shot").to_rgb8());
    }
    let at = |shot: &image::RgbImage, q: IVec2| shot.get_pixel(q.x as u32, q.y as u32).0;
    let apart = |a: [u8; 3], b: [u8; 3]| (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap_or(0);
    let mut worst = (0, IVec2::ZERO, 0);
    for out in 2..=5 {
        for q in pillar_ring(PILLARS[1], out) {
            let d = apart(at(&shots[0], q), at(&shots[1], q));
            if d > worst.0 {
                worst = (d, q, out);
            }
        }
    }
    eprintln!(
        "under temporal anti-aliasing the sky round the pillar's reflection is at most {} from \
         the drawn mirror's (at {}, {} out): drawn {:?}, far view {:?}",
        worst.0,
        worst.1,
        worst.2,
        at(&shots[0], worst.1),
        at(&shots[1], worst.1)
    );
    assert!(
        worst.0 <= 8,
        "the far view fringes the pillar's reflection under temporal anti-aliasing: {} apart \
         from the drawn mirror at {}",
        worst.0,
        worst.1
    );
}

/// A wall 180 m long, 50 m high and 2 m thick, edge on to the camera from 290 m off to 470 m,
/// right of the balls and above the top of the screen: its face toward the eye reflects across
/// the end of the rays' reach (384 m along them from the plane, about 370 m off).
const SEAM_WALL: DVec3 = DVec3::new(60.0, 135.0, -380.0);
const SEAM_WALL_SIZE: [f32; 3] = [2.0, 50.0, 180.0];

/// How far a texel's ray goes on from the mirror to `p` (m): from where the main camera's ray
/// to `p`'s reflection meets the plane.
fn reach_to(p: DVec3) -> f64 {
    let m = mirrored(p);
    let s = EYE + (m - EYE) * (EYE.y / (EYE.y - m.y));
    (p - s).length()
}

/// Where a ridge crosses the end of a traced mirror's reach, its far view
/// (`trial_cut_more::RAY_MIRROR_FAR`) takes over from the rays with no line and no gap: along a
/// wall's face from 300 m off to 420 m (short of its far end by more than a texel of the view)
/// the reflection is the wall throughout, as the drawn mirror shows it, and changes along the
/// face no more sharply than the drawn one's (the rays' hits blend into the view over the last
/// tenth of their reach). With the rays alone the face ends where they do.
#[test]
fn a_ridge_across_the_end_of_a_traced_mirrors_rays_shows_no_line_with_its_far_view() {
    let mut shots = Vec::new();
    for (traced, far, name) in [
        (false, false, "seam-drawn.png"),
        (true, false, "seam-traced.png"),
        (true, true, "seam-far-view.png"),
    ] {
        let mut app = ball_app(traced);
        if far {
            app.world.resource_mut::<RenderSettings>().trial_cuts_more |= RAY_MIRROR_FAR;
        }
        app.world.spawn((
            Transform::from_translation(SEAM_WALL),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: SEAM_WALL_SIZE,
                },
                StandardMaterial::color(0.75, 0.35, 0.05),
            ),
        ));
        app.update_n(5);
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("skipping: no GPU ({})", d.0);
            return;
        }
        let d = app.world.resource::<Diagnostics>().clone();
        if traced {
            if d.get("render.ray.supported") != Some(1.0) {
                eprintln!("skipping: the GPU cannot hold the ray scene (no ray queries)");
                return;
            }
            assert_eq!(
                d.get("render.ray.mirror_far"),
                Some(if far { 1.0 } else { 0.0 }),
                "{name}: its far view"
            );
        }
        let dir = std::env::temp_dir().join("engine-planar-reflections");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(name);
        capture_now(&mut app.world, &path).expect("screenshot");
        if let Ok(out) = std::env::var("ENGINE_SHOTS") {
            let _ = std::fs::copy(&path, std::path::Path::new(&out).join(name));
        }
        shots.push(image::open(&path).expect("read the shot").to_rgb8());
    }
    let (drawn, traced, far) = (&shots[0], &shots[1], &shots[2]);
    let face_x = SEAM_WALL.x - f64::from(SEAM_WALL_SIZE[0]) * 0.5;
    let bottom = SEAM_WALL.y - f64::from(SEAM_WALL_SIZE[1]) * 0.5;
    let half = f64::from(SEAM_WALL_SIZE[2]) * 0.5;
    for z in [SEAM_WALL.z - half, SEAM_WALL.z + half] {
        let foot = pixel(DVec3::new(face_x, bottom, z));
        assert!(foot.y < 0, "the wall is on screen at {foot}");
    }
    // Along the middle of its face, a pixel each, nearest first.
    let mut along: Vec<(f64, IVec2)> = Vec::new();
    for k in 0..=48 {
        let p = DVec3::new(face_x, SEAM_WALL.y, -300.0 - 2.5 * k as f64);
        let q = pixel(mirrored(p));
        if along.last().is_none_or(|&(_, last)| last != q) {
            along.push((reach_to(p), q));
        }
    }
    let at = |shot: &image::RgbImage, q: IVec2| shot.get_pixel(q.x as u32, q.y as u32).0;
    let orange = |c: [u8; 3]| c[0] > c[2].saturating_add(40) && c[1] > c[2];
    let apart = |a: [u8; 3], b: [u8; 3]| (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap_or(0);
    let (first, last) = (along[0].0, along[along.len() - 1].0);
    assert!(
        first < 340.0 && last > 420.0,
        "the face runs from {first:.0} m to {last:.0} m along the rays"
    );
    let mut most = (0, IVec2::ZERO);
    for &(t, q) in &along {
        let (d, r, f) = (at(drawn, q), at(traced, q), at(far, q));
        eprintln!("{t:5.0} m {q}: drawn {d:?}, traced {r:?}, far view {f:?}");
        assert!(orange(d), "drawn: the wall at {t:.0} m, {q}: {d:?}");
        assert!(orange(f), "far view: the wall at {t:.0} m, {q}: {f:?}");
        if t < 340.0 {
            assert!(orange(r), "traced: the wall at {t:.0} m, {q}: {r:?}");
        } else if t > 400.0 {
            assert!(!orange(r), "traced: past the rays at {t:.0} m, {q}: {r:?}");
        }
        let a = apart(d, f);
        if a > most.0 {
            most = (a, q);
        }
    }
    let step = |shot: &image::RgbImage| {
        along
            .windows(2)
            .map(|w| apart(at(shot, w[0].1), at(shot, w[1].1)))
            .max()
            .unwrap_or(0)
    };
    let (drawn_step, far_step) = (step(drawn), step(far));
    eprintln!(
        "along the face the far view's picture is at most {} from the drawn mirror's (at {}); \
         its sharpest step {far_step}, the drawn mirror's {drawn_step}",
        most.0, most.1
    );
    assert!(
        most.0 <= 24,
        "the far view's wall {} apart from the drawn mirror's at {}",
        most.0,
        most.1
    );
    assert!(
        far_step <= drawn_step + 8,
        "a line where the rays end: a step of {far_step} along the face, {drawn_step} drawn"
    );
}

/// The cover (`WeatherState::cloud_cover`) of the sky's own cloud layer (sky.wgsl `cloud_layer`,
/// on by default with `RenderSettings::sky_cloud_layer`) for
/// [`a_traced_mirrors_far_view_shows_the_clouds_it_drew_off_the_screen`]: most of the sky
/// clouded, with small clouds and ragged edges a degree or less across.
const CLOUD_COVER: f32 = 0.6;

/// Renders `traced` (with its far view where `far`) after `setup`, as `name`; None (with a note)
/// without a GPU or ray queries.
fn render_with(
    traced: bool,
    far: bool,
    name: &str,
    setup: impl FnOnce(&mut App),
) -> Option<image::RgbImage> {
    let mut app = ball_app(traced);
    if far {
        app.world.resource_mut::<RenderSettings>().trial_cuts_more |= RAY_MIRROR_FAR;
    }
    setup(&mut app);
    app.update_n(5);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let d = app.world.resource::<Diagnostics>().clone();
    if traced {
        if d.get("render.ray.supported") != Some(1.0) {
            eprintln!("skipping: the GPU cannot hold the ray scene (no ray queries)");
            return None;
        }
        assert_eq!(
            d.get("render.ray.mirror_pictures"),
            Some(1.0),
            "{name}: traced"
        );
        assert_eq!(
            d.get("render.ray.mirror_far"),
            Some(if far { 1.0 } else { 0.0 }),
            "{name}: its far view"
        );
    }
    let dir = std::env::temp_dir().join("engine-planar-reflections");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(name);
    capture_now(&mut app.world, &path).expect("screenshot");
    if let Ok(out) = std::env::var("ENGINE_SHOTS") {
        let _ = std::fs::copy(&path, std::path::Path::new(&out).join(name));
    }
    Some(image::open(&path).expect("read the shot").to_rgb8())
}

/// The mirror left of the balls' reflections, which shows the sky 11° to 37° up there, above
/// the top of the screen (10° up at most), under most of the clouds of [`CLOUD_COVER`].
const SKY_XS: std::ops::RangeInclusive<u32> = 30..=160;
const SKY_YS: std::ops::RangeInclusive<u32> = 120..=230;

/// Checks that [`SKY_XS`] by [`SKY_YS`] lies on the mirror and reflects the sky above the top of
/// the screen, at its four corners.
fn assert_sky_region_off_the_screen() {
    let toward = Transform::from_translation(EYE).looking_at(TARGET, Vec3::Y);
    let aspect = WINDOW.x as f32 / WINDOW.y as f32;
    let t = (FOV.to_radians() * 0.5).tan();
    for (x, y) in [
        (*SKY_XS.start(), *SKY_YS.start()),
        (*SKY_XS.end(), *SKY_YS.start()),
        (*SKY_XS.start(), *SKY_YS.end()),
        (*SKY_XS.end(), *SKY_YS.end()),
    ] {
        let ndc = Vec2::new(
            (x as f32 + 0.5) / WINDOW.x as f32 * 2.0 - 1.0,
            1.0 - (y as f32 + 0.5) / WINDOW.y as f32 * 2.0,
        );
        let ray = toward.rotation * Vec3::new(ndc.x * t * aspect, ndc.y * t, -1.0);
        // Where the pixel's ray meets the mirror, and the sky's elevation it reflects (°).
        let on = EYE + ray.as_dvec3() * (EYE.y / -f64::from(ray.y));
        let up = (-ray.normalize().y).asin().to_degrees();
        assert!(
            on.x.abs() < f64::from(SIZE) * 0.5 && on.z.abs() < f64::from(SIZE) * 0.5 && up > 10.5,
            "pixel {x}, {y} sees {on} on the mirror, the sky {up:.1}° up"
        );
    }
}

/// How far `a` stands from `b` over [`SKY_XS`] by [`SKY_YS`], on average (levels of 255).
fn sky_apart(a: &image::RgbImage, b: &image::RgbImage) -> f64 {
    let mut sum = 0.0;
    let mut n = 0.0;
    for y in SKY_YS {
        for x in SKY_XS {
            let (p, q) = (a.get_pixel(x, y).0, b.get_pixel(x, y).0);
            for c in 0..3 {
                sum += f64::from(p[c].abs_diff(q[c]));
            }
            n += 3.0;
        }
    }
    sum / n
}

/// The clouds' detail over [`SKY_XS`] by [`SKY_YS`]: how far each pixel's brightness stands from
/// the mean of the 9 by 9 round it, on average. The sky's smooth rise toward the horizon and the
/// clouds' broad shapes, which the environment map keeps, add next to nothing.
fn sky_detail(a: &image::RgbImage) -> f64 {
    let luma = |x: u32, y: u32| {
        let p = a.get_pixel(x, y).0.map(f64::from);
        0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]
    };
    let mut sum = 0.0;
    let mut n = 0.0;
    for y in SKY_YS {
        for x in SKY_XS {
            let mut round = 0.0;
            for dy in 0..9 {
                for dx in 0..9 {
                    round += luma(x + dx - 4, y + dy - 4) / 81.0;
                }
            }
            sum += (luma(x, y) - round).abs();
            n += 1.0;
        }
    }
    sum / n
}

/// The sky a traced mirror shows with its far view (`trial_cut_more::RAY_MIRROR_FAR`), where the
/// screen doesn't show it, is the one the view drew there, small clouds and all, as the drawn
/// mirror shows it. Across the reflection of the clouded sky above the top of the screen, with
/// the view at the picture's own size (`RenderSettings::ray_mirror_far_scale` 1, so its sky is
/// the drawn mirror's to the texel), the far view's picture stands closer to the drawn
/// mirror's than the environment map's does and keeps most of the clouds' detail (over a clear
/// sky's), where the environment map, a 256 by 128 picture, keeps under half. These are the sky
/// pass's own clouds, drawn per pixel; the weather's, marched by the view's pixels, march as
/// finely in a far view of any size as in the drawn mirror's view, as it asks for the picture's
/// detail ([`a_traced_mirrors_far_view_shows_what_lies_past_its_rays_off_the_screen`]). No fog,
/// which the environment map's sky hasn't
/// ([`a_traced_mirrors_far_view_shows_the_haze_it_drew_off_the_screen`]), and no wind, which
/// would move the clouds by each run's own time.
#[test]
fn a_traced_mirrors_far_view_shows_the_clouds_it_drew_off_the_screen() {
    assert_sky_region_off_the_screen();
    let mut shots = Vec::new();
    for (traced, far, cover, name) in [
        (false, false, CLOUD_COVER, "clouds-drawn.png"),
        (true, false, CLOUD_COVER, "clouds-traced.png"),
        (true, true, CLOUD_COVER, "clouds-far-view.png"),
        (false, false, 0.0, "clouds-none-drawn.png"),
    ] {
        let Some(shot) = render_with(traced, far, name, |app| {
            app.world.insert_resource(WeatherState {
                cloud_cover: cover,
                fog: 0.0,
                wind: Vec3::ZERO,
                ..Default::default()
            });
            app.world.resource_mut::<FogSettings>().density = 0.0;
            app.world
                .resource_mut::<RenderSettings>()
                .ray_mirror_far_scale = 1.0;
        }) else {
            return;
        };
        assert_reflects_what_is_above(&shot, name);
        shots.push(shot);
    }
    let (drawn, traced, far, clear) = (&shots[0], &shots[1], &shots[2], &shots[3]);
    let (far_off, traced_off) = (sky_apart(far, drawn), sky_apart(traced, drawn));
    // The clouds' own detail: over the clear sky's.
    let base = sky_detail(clear);
    let [drawn_detail, far_detail, traced_detail] =
        [drawn, far, traced].map(|a| sky_detail(a) - base);
    eprintln!(
        "over the clouded sky's reflection the far view's picture is {far_off:.2} from the drawn \
         mirror's on average, the rays alone's {traced_off:.2} (the clouds themselves {:.2}); \
         the clouds' detail is {drawn_detail:.2} drawn, {far_detail:.2} with the far view, \
         {traced_detail:.2} with the rays alone, over the clear sky's {base:.2}",
        sky_apart(clear, drawn)
    );
    assert!(
        drawn_detail > 0.4,
        "the drawn mirror shows clouds: their detail {drawn_detail:.2} over the clear sky's"
    );
    assert!(
        far_off < 0.5 * traced_off,
        "the far view's clouds stand {far_off:.2} from the drawn mirror's, the environment \
         map's {traced_off:.2}"
    );
    assert!(
        far_detail > 0.75 * drawn_detail && traced_detail < 0.5 * drawn_detail,
        "the far view keeps the clouds' detail, {far_detail:.2} of the drawn mirror's \
         {drawn_detail:.2}, the environment map less: {traced_detail:.2}"
    );
}

/// The traced mirror's sky from its far view is also fogged as the drawn mirror's: under the
/// default haze and the weather's ground fog, with a clear sky and no wind, the far view's
/// picture of the sky above the top of the screen stands close to the drawn mirror's, and the
/// environment map's (unfogged) far apart: in the cone round the sun it left a hole about 20
/// levels darker than the sky round it.
#[test]
fn a_traced_mirrors_far_view_shows_the_haze_it_drew_off_the_screen() {
    assert_sky_region_off_the_screen();
    let mut shots = Vec::new();
    for (traced, far, name) in [
        (false, false, "haze-drawn.png"),
        (true, false, "haze-traced.png"),
        (true, true, "haze-far-view.png"),
    ] {
        let Some(shot) = render_with(traced, far, name, |app| {
            app.world.insert_resource(WeatherState {
                cloud_cover: 0.0,
                wind: Vec3::ZERO,
                ..Default::default()
            });
        }) else {
            return;
        };
        assert_reflects_what_is_above(&shot, name);
        shots.push(shot);
    }
    let (drawn, traced, far) = (&shots[0], &shots[1], &shots[2]);
    let (far_off, traced_off) = (sky_apart(far, drawn), sky_apart(traced, drawn));
    eprintln!(
        "over the hazy sky's reflection the far view's picture is {far_off:.2} from the drawn \
         mirror's on average, the rays alone's {traced_off:.2}"
    );
    assert!(
        far_off < 0.2 * traced_off,
        "the far view's haze stands {far_off:.2} from the drawn mirror's, the environment \
         map's {traced_off:.2}"
    );
}

/// Toward the sun for [`a_traced_mirror_shows_no_disc_of_the_sun`] with the camera as
/// [`ball_app`] has it: 30° up and 30° left of straight ahead, above the top of the screen (10°
/// up at most), its reflection on the mirror left of the balls'.
const SUN_ABOVE: DVec3 = DVec3::new(-0.433, 0.5, -0.75);
/// Where the camera looks instead for the sun on the screen: level, ahead.
const LEVEL: DVec3 = DVec3::new(0.0, 2.5, -10.0);
/// Toward the sun with the camera level: 18° up and 25° left, on the screen (30° up at most),
/// its reflection on the mirror (below 9.5° down) and the rings round that on the screen.
const SUN_SEEN: DVec3 = DVec3::new(-0.402, 0.309, -0.862);
/// The sun's disc across, degrees: a few of the far view's texels, and the cone the traced
/// picture keeps it out of (three times its radius) with a ring round it on the mirror.
const SUN_DISC_DEG: f32 = 4.0;
/// A pixel this bright in every channel is the sun's disc or the glow round it, the disc as the
/// traced picture holds it too (it keeps the sky to 60 000 cd/m², about 226 here): the sky
/// round the sun is darker.
const SUN_BRIGHT: u8 = 200;

/// Where the camera looking from [`EYE`] at `target` sees `p`, pixels.
fn pixel_toward(target: DVec3, p: DVec3) -> IVec2 {
    pixel_from(EYE, target, p)
}

/// Where the camera looking from `eye` at `target` sees `p`, pixels.
fn pixel_from(eye: DVec3, target: DVec3, p: DVec3) -> IVec2 {
    let view = Transform::from_translation(eye).looking_at(target, Vec3::Y);
    let v = view.rotation.inverse() * (p - eye).as_vec3();
    let aspect = WINDOW.x as f32 / WINDOW.y as f32;
    let f = 1.0 / (FOV.to_radians() * 0.5).tan();
    let ndc = Vec2::new(f / aspect * v.x, f * v.y) / -v.z;
    IVec2::new(
        ((ndc.x + 1.0) * 0.5 * WINDOW.x as f32).floor() as i32,
        ((1.0 - ndc.y) * 0.5 * WINDOW.y as f32).floor() as i32,
    )
}

/// Where the camera looking from [`EYE`] at `target` sees the 24 directions `angle` (degrees)
/// from unit `about`, round it.
fn ring_toward(target: DVec3, about: DVec3, angle: f64) -> Vec<IVec2> {
    let u = about.cross(DVec3::Y).normalize();
    let v = about.cross(u);
    let (s, c) = angle.to_radians().sin_cos();
    (0..24)
        .map(|k| {
            let phi = f64::from(k) / 24.0 * std::f64::consts::TAU;
            let d = about * c + (u * phi.cos() + v * phi.sin()) * s;
            pixel_toward(target, EYE + d * 1000.0)
        })
        .collect()
}

/// The pixels as bright as the sun within 40 of `at`: how many, and their middle.
fn sun_blob(shot: &image::RgbImage, at: IVec2) -> (u32, DVec2) {
    let (mut n, mut sum) = (0u32, DVec2::ZERO);
    for dy in -40..=40 {
        for dx in -40..=40 {
            let p = at + IVec2::new(dx, dy);
            if p.cmplt(IVec2::ZERO).any() || p.cmpge(WINDOW.as_ivec2()).any() {
                continue;
            }
            if shot
                .get_pixel(p.x as u32, p.y as u32)
                .0
                .iter()
                .all(|&v| v >= SUN_BRIGHT)
            {
                n += 1;
                sum += p.as_dvec2();
            }
        }
    }
    (
        n,
        if n > 0 {
            sum / f64::from(n)
        } else {
            DVec2::ZERO
        },
    )
}

/// A traced mirror leaves the sun's disc out of its picture, from the screen and from its far
/// view (`trial_cut_more::RAY_MIRROR_FAR`) alike, where the drawn mirror shows it (its view
/// draws the sky with the disc): the water's glitter is the sun's reflection, so a disc in the
/// picture would be the sun a second time. With the sun above the top of the screen the far
/// view's sky holds the disc; with the camera level the screen shows the sun, which the
/// picture would read there. Within three times the sun's radius of its reflection the traced
/// pictures take the sky just past that cone, so it shows no hole either: as the sky just
/// outside it, where the environment map's sky stood about 20 levels darker than the fogged sky
/// round it.
#[test]
fn a_traced_mirror_shows_no_disc_of_the_sun() {
    let sun = |toward: DVec3| {
        move |app: &mut App| {
            let mut q = app.world.query::<(&mut DirectionalLight, &mut Transform)>();
            for (mut light, mut at) in q.iter_mut(&mut app.world) {
                light.angular_size_deg = SUN_DISC_DEG;
                *at = Transform::default().looking_at(-toward, Vec3::Y);
            }
        }
    };
    let level = |toward: DVec3| {
        let lit = sun(toward);
        move |app: &mut App| {
            lit(app);
            let mut q = app.world.query::<(&Camera, &mut Transform)>();
            for (_, mut at) in q.iter_mut(&mut app.world) {
                *at = Transform::from_translation(EYE).looking_at(LEVEL, Vec3::Y);
            }
        }
    };
    let mut shots = Vec::new();
    for (traced, far, name, camera) in [
        (false, false, "sun-above-drawn.png", TARGET),
        (true, true, "sun-above-far-view.png", TARGET),
        (false, false, "sun-seen-drawn.png", LEVEL),
        (true, false, "sun-seen-traced.png", LEVEL),
    ] {
        let shot = if camera == TARGET {
            render_with(traced, far, name, sun(SUN_ABOVE))
        } else {
            render_with(traced, far, name, level(SUN_SEEN))
        };
        let Some(shot) = shot else {
            return;
        };
        shots.push((shot, camera));
    }
    for (toward, camera, shown) in [(SUN_ABOVE, TARGET, 0), (SUN_SEEN, LEVEL, 2)] {
        let toward = toward.normalize();
        let sun_at = pixel_toward(camera, EYE + toward * 1000.0);
        let seen = sun_at.cmpge(IVec2::ZERO).all() && sun_at.cmplt(WINDOW.as_ivec2()).all();
        assert_eq!(
            seen,
            camera == LEVEL,
            "the sun at {sun_at} with the camera toward {camera}"
        );
        // Its reflection: where the camera's ray along its direction mirrored meets the mirror,
        // well inside it.
        let down = mirrored(toward);
        let on = EYE + down * (EYE.y / -down.y);
        assert!(
            on.x.abs() < f64::from(SIZE) * 0.4 && on.z.abs() < f64::from(SIZE) * 0.4,
            "the sun's reflection meets the mirror at {on}"
        );
        let q = pixel_toward(camera, EYE + down * 1000.0);
        let (drawn, traced) = (&shots[shown].0, &shots[shown + 1].0);
        let (d, t) = (sun_blob(drawn, q), sun_blob(traced, q));
        let middle = |shot: &image::RgbImage| shot.get_pixel(q.x as u32, q.y as u32).0;
        eprintln!(
            "the sun's reflection at {q} (the sun {}): drawn {:?}, {} bright pixels round {:.1}; \
             traced {:?}, {} round {:.1}",
            if seen {
                "on screen"
            } else {
                "above the screen"
            },
            middle(drawn),
            d.0,
            d.1,
            middle(traced),
            t.0,
            t.1
        );
        assert!(
            d.0 > 100 && d.1.distance(q.as_dvec2()) < 3.0,
            "the drawn mirror shows the sun at {q}: {} bright pixels round {:.1}",
            d.0,
            d.1
        );
        assert_eq!(
            t.0,
            0,
            "the traced mirror shows the sun's disc at {q} (the sun {}): {} bright pixels round \
             {:.1}",
            if seen {
                "on screen"
            } else {
                "above the screen"
            },
            t.0,
            t.1
        );
        // No hole round it: inside the cone the traced picture stands as the sky just outside.
        let cone = 1.5 * f64::from(SUN_DISC_DEG);
        let mean = |ring: &[IVec2]| {
            let mut sum = [0.0f64; 3];
            for p in ring {
                assert!(
                    p.cmpge(IVec2::ZERO).all() && p.cmplt(WINDOW.as_ivec2()).all(),
                    "the ring round the sun's reflection leaves the screen at {p}"
                );
                let c = traced.get_pixel(p.x as u32, p.y as u32).0;
                for (s, v) in sum.iter_mut().zip(c) {
                    *s += f64::from(v) / ring.len() as f64;
                }
            }
            sum
        };
        let inside = mean(&ring_toward(camera, down, 0.8 * cone));
        let outside = mean(&ring_toward(camera, down, cone + 2.0));
        let step = (0..3)
            .map(|c| (inside[c] - outside[c]).abs())
            .fold(0.0, f64::max);
        eprintln!(
            "round it the traced picture is {inside:.1?} inside the cone and {outside:.1?} just \
             outside: {step:.1} apart"
        );
        assert!(
            step < 8.0,
            "a hole round the sun's reflection at {q}: {inside:.1?} inside the cone, {outside:.1?} \
             just outside"
        );
        if seen {
            // The screen shows the sun itself, in both pictures.
            for (shot, name) in [(drawn, "drawn"), (traced, "traced")] {
                let s = sun_blob(shot, sun_at);
                assert!(
                    s.0 > 100,
                    "{name}: the sun on the screen at {sun_at}: {} bright pixels",
                    s.0
                );
            }
        }
    }
}

/// The camera's eye for [`a_traced_mirror_under_a_low_sun_shows_the_sky_beside_it_not_the_ground`]:
/// 0.3 m above the mirror, which then shows the sky down to about 1° up past the sun's
/// reflection.
const LOW_EYE: DVec3 = DVec3::new(0.0, 0.3, 5.0);
/// Where it looks: level, ahead (the screen shows the sun and the sky round it)...
const LOW_LEVEL: DVec3 = DVec3::new(0.0, 0.3, -10.0);
/// ...or pitched down 30°, the top of the screen on the horizon and the sun above it (the far
/// view's sky).
const LOW_PITCHED: DVec3 = DVec3::new(0.0, -4.7, -3.66);
/// Toward the sun: 4° up and 15° left.
const SUN_LOW: DVec3 = DVec3::new(-0.2582, 0.06976, -0.9636);

/// Under a low sun the sky a traced mirror takes round its disc is the sky beside it, not the
/// ground: the way straight off the sun from a direction just under it leads under the plane,
/// where the screen, the far view and the environment map show the ground below the horizon.
/// Between the sun's reflection and the mirror's far edge, inside the cone round the sun, the
/// traced picture is no darker than the sky's reflection beside the cone at the same height.
#[test]
fn a_traced_mirror_under_a_low_sun_shows_the_sky_beside_it_not_the_ground() {
    let setup = |target: DVec3| {
        move |app: &mut App| {
            let mut q = app.world.query::<(&mut DirectionalLight, &mut Transform)>();
            for (mut light, mut at) in q.iter_mut(&mut app.world) {
                light.angular_size_deg = SUN_DISC_DEG;
                *at = Transform::default().looking_at(-SUN_LOW, Vec3::Y);
            }
            let mut q = app.world.query::<(&Camera, &mut Transform)>();
            for (_, mut at) in q.iter_mut(&mut app.world) {
                *at = Transform::from_translation(LOW_EYE).looking_at(target, Vec3::Y);
            }
        }
    };
    let mut shots = Vec::new();
    for (far, name, target) in [
        (false, "low-sun-traced.png", LOW_LEVEL),
        (true, "low-sun-far-view.png", LOW_PITCHED),
    ] {
        let Some(shot) = render_with(true, far, name, setup(target)) else {
            return;
        };
        shots.push((shot, target));
    }
    // The sky `up` (°) high and `left` (°) left of ahead.
    let toward = |up: f64, left: f64| {
        let (up, left) = (up.to_radians(), left.to_radians());
        DVec3::new(-up.cos() * left.sin(), up.sin(), -up.cos() * left.cos())
    };
    let sun = SUN_LOW.normalize();
    for ((shot, target), what) in shots.iter().zip(["the screen's sky", "the far view's sky"]) {
        // The mean colour of the picture where the mirror reflects the sky 1.4° to 2° up,
        // around `left`.
        let patch = |left: f64| {
            let mut sum = [0.0f64; 3];
            for up in [1.4, 1.7, 2.0] {
                for off in [-0.5, 0.0, 0.5] {
                    let d = toward(up, left + off);
                    let on = LOW_EYE + mirrored(d) * (LOW_EYE.y / d.y);
                    let p = pixel_from(LOW_EYE, *target, LOW_EYE + mirrored(d) * 1000.0);
                    assert!(
                        on.x.abs() < f64::from(SIZE) * 0.45
                            && on.z.abs() < f64::from(SIZE) * 0.45
                            && p.cmpge(IVec2::ZERO).all()
                            && p.cmplt(WINDOW.as_ivec2()).all(),
                        "the sky {up}° up and {}° left reflects at {on} on the mirror, {p} on \
                         the screen",
                        left + off
                    );
                    let c = shot.get_pixel(p.x as u32, p.y as u32).0;
                    for (s, v) in sum.iter_mut().zip(c) {
                        *s += f64::from(v) / 9.0;
                    }
                }
            }
            sum
        };
        let luma = |c: [f64; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        let under = patch(15.0);
        let (left, right) = (patch(25.0), patch(5.0));
        let beside = [0, 1, 2].map(|c| 0.5 * (left[c] + right[c]));
        eprintln!(
            "{what}: under the low sun's reflection the traced picture is {under:.1?} (the sky \
             {:.1}° from the sun), beside it {beside:.1?}",
            toward(1.7, 15.0).angle_between(sun).to_degrees()
        );
        assert!(
            luma(under) > luma(beside) - 8.0,
            "{what}: under the low sun's reflection the traced picture is {under:.1?}, darker than \
             the sky beside it, {beside:.1?}"
        );
    }
}

/// Toward the sun for [`a_traced_mirror_shows_no_sun_on_the_crest_of_a_wall_in_front_of_it`]:
/// 10° up and 25° left of the level camera's way, clear of the balls.
const SUN_CREST: DVec3 = DVec3::new(-0.4162, 0.17365, -0.8925);
/// How far off the camera the wall in front of that sun stands (its face, along the sun's way).
const CREST_WALL_OFF: f64 = 30.0;

/// The camera and the mirrored eye, 5 m apart (twice the camera's height over the mirror), see
/// the crest of a wall 30 m off in front of the sun about 9° apart: more than the cone round the
/// sun (three times its radius, 6° here). With the camera level the sun sits half behind the
/// crest, and the screen's picture blends its disc into the crest's outline. A traced texel
/// whose ray hits the crest takes last frame's picture where the camera sees the hit, though its
/// own ray passes 9° clear of the sun. The drawn mirror shows the wall there (from the mirrored
/// eye the crest hides the sun), and so must the traced picture: the screen's answer is held to
/// half again the sky wherever the camera's line to the hit lies within the cone, as well as
/// where the texel's ray does.
#[test]
fn a_traced_mirror_shows_no_sun_on_the_crest_of_a_wall_in_front_of_it() {
    let toward = SUN_CREST.normalize();
    let flat = DVec3::new(toward.x, 0.0, toward.z).normalize();
    // The crest of the wall's face, on the sun's middle as the camera sees it.
    let crest = DVec3::new(EYE.x, 0.0, EYE.z)
        + flat * CREST_WALL_OFF
        + DVec3::Y * (EYE.y + CREST_WALL_OFF * toward.y / toward.x.hypot(toward.z));
    let setup = move |app: &mut App| {
        let mut q = app.world.query::<(&mut DirectionalLight, &mut Transform)>();
        for (mut light, mut at) in q.iter_mut(&mut app.world) {
            light.angular_size_deg = SUN_DISC_DEG;
            *at = Transform::default().looking_at(-toward, Vec3::Y);
        }
        let mut q = app.world.query::<(&Camera, &mut Transform)>();
        for (_, mut at) in q.iter_mut(&mut app.world) {
            *at = Transform::from_translation(EYE).looking_at(LEVEL, Vec3::Y);
        }
        // 12 m wide and 1 m thick, from the ground to the crest, facing the camera.
        let middle = DVec3::new(crest.x, 0.5 * crest.y, crest.z) + flat * 0.5;
        app.world.spawn((
            Transform::from_translation(middle).looking_at(middle + flat, Vec3::Y),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [12.0, crest.y as f32, 1.0],
                },
                StandardMaterial::color(0.4, 0.4, 0.4),
            ),
        ));
    };
    let mut shots = Vec::new();
    for (traced, name) in [(false, "crest-drawn.png"), (true, "crest-traced.png")] {
        let Some(shot) = render_with(traced, false, name, setup) else {
            return;
        };
        shots.push(shot);
    }
    let (drawn, traced) = (&shots[0], &shots[1]);
    let sun_at = pixel_from(EYE, LEVEL, EYE + toward * 1000.0);
    // The crest's reflection: where the camera sees the crest mirrored, on the mirror.
    let reflection = pixel_from(EYE, LEVEL, mirrored(crest));
    let on = EYE + (mirrored(crest) - EYE) * (EYE.y / (EYE.y + crest.y));
    assert!(
        on.x.abs() < f64::from(SIZE) * 0.45 && on.z.abs() < f64::from(SIZE) * 0.45,
        "the crest's reflection meets the mirror at {on}"
    );
    let below = (EYE.y + crest.y).atan2(CREST_WALL_OFF);
    eprintln!(
        "the crest {:.1}° up from the camera (the sun's middle {:.1}°), {:.1}° from the mirrored \
         eye; the sun at {sun_at}, the crest's reflection at {reflection}",
        (crest.y - EYE.y).atan2(CREST_WALL_OFF).to_degrees(),
        toward.y.asin().to_degrees(),
        below.to_degrees()
    );
    for (shot, name) in [(drawn, "drawn"), (traced, "traced")] {
        // The camera sees the sun's upper half over the crest.
        let (n, middle) = sun_blob(shot, sun_at);
        assert!(
            n > 40 && middle.y < f64::from(sun_at.y) - 1.0,
            "{name}: the sun half over the crest at {sun_at}: {n} bright pixels round {middle:.1}"
        );
        // Nothing as bright as the sun round the crest's reflection.
        let (n, middle) = sun_blob(shot, reflection);
        let brightest = (-40..=40)
            .flat_map(|dy| (-40..=40).map(move |dx| reflection + IVec2::new(dx, dy)))
            .filter(|p| p.cmpge(IVec2::ZERO).all() && p.cmplt(WINDOW.as_ivec2()).all())
            .map(|p| shot.get_pixel(p.x as u32, p.y as u32).0)
            .max_by_key(|c| c.iter().map(|&v| u32::from(v)).sum::<u32>())
            .unwrap_or_default();
        eprintln!(
            "{name}: round the crest's reflection {n} pixels as bright as the sun, the brightest \
             {brightest:?}"
        );
        assert_eq!(
            n, 0,
            "{name}: the sun on the crest's reflection at {reflection}: {n} bright pixels round \
             {middle:.1}"
        );
    }
}

/// A wall 400 m wide and 60 m high, 120 m off, within the rays' reach and above the top of the
/// screen: only its reflection shows, by the mirror's middle.
const FOG_WALL: DVec3 = DVec3::new(0.0, 60.0, -120.0);
/// A point of its face, left of the balls, whose reflection is on the mirror.
const FOG_WALL_POINT: DVec3 = DVec3::new(-30.0, 45.0, -118.0);

/// What the rays light in a traced mirror is fogged over its mirrored path, from the mirrored
/// eye through the plane, as the drawn mirror fogs what it draws: in thick fog a wall within
/// the rays' reach, above the top of the screen, reflects about as the drawn mirror shows it,
/// where the traced picture without the fog's share of it stands far apart.
#[test]
fn a_traced_mirrors_hits_are_fogged_as_the_drawn_mirrors_are() {
    let mut shots = Vec::new();
    for (traced, fog, name) in [
        (false, true, "fog-wall-drawn.png"),
        (true, true, "fog-wall-traced.png"),
        (true, false, "fog-wall-traced-clear.png"),
    ] {
        let mut app = ball_app(traced);
        // The height fog alone, analytic in both pictures (the froxels' fog is lit by the
        // shadow maps, which the trace doesn't read), 100 m deep at the mirror.
        app.world.resource_mut::<RenderSettings>().volumetric_fog = false;
        app.world.resource_mut::<FogSettings>().density = if fog { 0.01 } else { 0.0 };
        app.world.spawn((
            Transform::from_translation(FOG_WALL),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [400.0, 60.0, 4.0],
                },
                StandardMaterial::color(0.75, 0.35, 0.05),
            ),
        ));
        app.update_n(5);
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("skipping: no GPU ({})", d.0);
            return;
        }
        let d = app.world.resource::<Diagnostics>().clone();
        if traced {
            if d.get("render.ray.supported") != Some(1.0) {
                eprintln!("skipping: the GPU cannot hold the ray scene (no ray queries)");
                return;
            }
            assert_eq!(
                d.get("render.ray.mirror_pictures"),
                Some(1.0),
                "{name}: traced"
            );
        }
        let dir = std::env::temp_dir().join("engine-planar-reflections");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(name);
        capture_now(&mut app.world, &path).expect("screenshot");
        if let Ok(out) = std::env::var("ENGINE_SHOTS") {
            let _ = std::fs::copy(&path, std::path::Path::new(&out).join(name));
        }
        shots.push(image::open(&path).expect("read the shot").to_rgb8());
    }
    let foot = DVec3::new(FOG_WALL_POINT.x, FOG_WALL.y - 30.0, FOG_WALL_POINT.z);
    assert!(
        pixel(foot).y < 0,
        "the wall is on screen at {}",
        pixel(foot)
    );
    // The mean colour over the 3 x 3 pixels around the wall's reflection, and how far apart two
    // are in the channel that differs most (the fog turns the orange wall toward its own grey
    // blue: the blue climbs and the red falls, whatever the mean of the three does).
    let q = pixel(mirrored(FOG_WALL_POINT));
    let colour = |shot: &image::RgbImage| {
        let mut sum = [0.0f32; 3];
        for dy in -1..=1 {
            for dx in -1..=1 {
                let p = shot.get_pixel((q.x + dx) as u32, (q.y + dy) as u32).0;
                for (s, v) in sum.iter_mut().zip(p) {
                    *s += f32::from(v) / 9.0;
                }
            }
        }
        sum
    };
    let apart = |a: [f32; 3], b: [f32; 3]| (0..3).map(|c| (a[c] - b[c]).abs()).fold(0.0, f32::max);
    let (drawn, traced, clear) = (colour(&shots[0]), colour(&shots[1]), colour(&shots[2]));
    eprintln!(
        "the wall's reflection at {q} in fog: drawn {drawn:.1?}, traced {traced:.1?}; traced \
         without the fog {clear:.1?}"
    );
    let (fogged, off) = (apart(clear, drawn), apart(traced, drawn));
    assert!(
        fogged > 20.0,
        "the fog changes the wall's reflection: {drawn:.1?} drawn in fog, {clear:.1?} traced \
         without it"
    );
    assert!(
        off < 0.2 * fogged,
        "the traced reflection in fog ({traced:.1?}) is fogged as the drawn one ({drawn:.1?}): \
         {off:.1} apart, and {fogged:.1} without the fog ({clear:.1?})"
    );
}

/// Above the top of the screen beside the yellow ball, so that only their reflections show: a
/// glossy ball and a matte one of one dark blue (`a_glossy_thing_shines_in_a_traced_mirror_as_in_a_drawn_one`).
const GLOSSY: DVec3 = DVec3::new(-2.6, 6.0, -10.0);
const MATTE: DVec3 = DVec3::new(2.6, 6.0, -10.0);

/// [`ball_app`] with the glossy ball and the matte one.
fn shine_app(traced: bool) -> App {
    let mut app = ball_app(traced);
    for (at, roughness) in [(GLOSSY, 0.15), (MATTE, 0.95)] {
        app.world.spawn((
            Transform::from_translation(at),
            MeshRenderer::new(
                Shape::Sphere {
                    radius: 0.8,
                    segments: 48,
                },
                StandardMaterial {
                    perceptual_roughness: roughness,
                    ..StandardMaterial::color(0.05, 0.08, 0.35)
                },
            ),
        ));
    }
    app
}

/// A reflected thing's own shine (ray_hit_light.wgsl `reflected_hit_lit`): in the mirror, the
/// sun's highlight on a glossy ball that only the reflection shows, where a matte ball of the
/// same colour beside it has none, traced by the frame's rays as the drawn mirror shows it. Lit
/// without its shine, a hit of the rays looked matte, and the two balls alike. And the lake's
/// far side shows nothing of the balls peeking in at the top of the screen (ray_reflect.wgsl
/// `full_march`'s crossing rule, ray_mirror.wgsl `mirror_sky_around`).
#[test]
fn a_glossy_thing_shines_in_a_traced_mirror_as_in_a_drawn_one() {
    for (traced, what) in [(false, "drawn"), (true, "traced")] {
        let mut app = shine_app(traced);
        app.update_n(5);
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("skipping: no GPU ({})", d.0);
            return;
        }
        let d = app.world.resource::<Diagnostics>();
        if traced {
            if d.get("render.ray.supported") != Some(1.0) {
                eprintln!("skipping: the GPU cannot hold the ray scene (no ray queries)");
                return;
            }
            assert_eq!(
                d.get("render.ray.mirror_pictures"),
                Some(1.0),
                "a traced picture"
            );
        }
        assert!(
            pixel(GLOSSY).y < 0 && pixel(MATTE).y < 0,
            "the balls are on screen"
        );
        let dir = std::env::temp_dir().join("engine-planar-reflections");
        let _ = std::fs::create_dir_all(&dir);
        let name = format!("shine-{what}.png");
        let path = dir.join(&name);
        capture_now(&mut app.world, &path).expect("screenshot");
        if let Ok(out) = std::env::var("ENGINE_SHOTS") {
            let _ = std::fs::copy(&path, std::path::Path::new(&out).join(&name));
        }
        let shot = image::open(&path).expect("read the shot").to_rgb8();
        // The whitest pixel of a ball's reflection (its least channel: the ball's own colour
        // has next to no red, a highlight and the sky it reflects have).
        let whitest = |p: DVec3| {
            let c = pixel(mirrored(p));
            let mut most = 0;
            for y in c.y - 10..=c.y + 10 {
                for x in c.x - 10..=c.x + 10 {
                    let q = shot.get_pixel(x as u32, y as u32).0;
                    most = most.max(q.iter().copied().min().unwrap_or(0));
                }
            }
            i32::from(most)
        };
        let (glossy, matte) = (whitest(GLOSSY), whitest(MATTE));
        // The lake's far side below where the balls peek in at the top of the screen: a ray
        // reflected there rises toward them on screen but passes far behind them, so nothing
        // of theirs shows (pixels darker than the stretch's median by more than 10 levels:
        // 9 and 14 before, under the march's old crossing rule and a filtered read of the
        // sky beside the balls' outlines, now in `mirror_screen_sky`; 5 and 6 with the
        // march's rule alone; 0 in the drawn mirror).
        let specks: usize = [(140u32, 172u32), (228, 260)]
            .iter()
            .map(|&(x0, x1)| {
                let lum: Vec<f32> = (102..114)
                    .flat_map(|y| (x0..x1).map(move |x| (x, y)))
                    .map(|(x, y)| {
                        let q = shot.get_pixel(x, y).0;
                        q.iter().map(|&c| f32::from(c)).sum::<f32>() / 3.0
                    })
                    .collect();
                let mut sorted = lum.clone();
                sorted.sort_by(f32::total_cmp);
                let median = sorted[sorted.len() / 2];
                lum.iter().filter(|&&l| l < median - 10.0).count()
            })
            .sum();
        eprintln!(
            "{what}: the glossy ball's reflection at its whitest {glossy}, the matte one's \
             {matte}; specks on the lake's far side {specks}"
        );
        assert!(
            glossy > matte + 60,
            "{what}: the glossy ball's reflection shines no more than the matte one's: {glossy} against {matte}"
        );
        assert_eq!(specks, 0, "{what}: specks on the lake's far side");
    }
}

/// Where the glossy and the matte ball are in [`shine_app`], above the top of the screen: a
/// gold ball and a chrome one, both bare metal
/// (`a_metal_shines_in_its_own_colour_in_a_traced_mirror_as_in_a_drawn_one`).
const GOLD: DVec3 = GLOSSY;
const CHROME: DVec3 = MATTE;

/// [`ball_app`] with the gold ball and the chrome one.
fn metal_app(traced: bool) -> App {
    let mut app = ball_app(traced);
    for (at, (r, g, b)) in [(GOLD, (1.0, 0.77, 0.34)), (CHROME, (0.9, 0.9, 0.9))] {
        app.world.spawn((
            Transform::from_translation(at),
            MeshRenderer::new(
                Shape::Sphere {
                    radius: 0.8,
                    segments: 48,
                },
                StandardMaterial {
                    perceptual_roughness: 0.25,
                    ..StandardMaterial::color(r, g, b).with_metallic(1.0)
                },
            ),
        ));
    }
    app
}

/// A reflected metal shines in its own colour (ray_hit_light.wgsl `reflected_hit_lit`: its F0
/// is its bare metal's, `RaySurface::metal`, the row's colour of it): in the mirror, a gold
/// ball's reflection is yellower than a chrome one's beside it, traced by the frame's rays as
/// the drawn mirror shows it. With one grey F0 for every metal the two looked alike.
#[test]
fn a_metal_shines_in_its_own_colour_in_a_traced_mirror_as_in_a_drawn_one() {
    for (traced, what) in [(false, "drawn"), (true, "traced")] {
        let mut app = metal_app(traced);
        app.update_n(5);
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("skipping: no GPU ({})", d.0);
            return;
        }
        let d = app.world.resource::<Diagnostics>();
        if traced {
            if d.get("render.ray.supported") != Some(1.0) {
                eprintln!("skipping: the GPU cannot hold the ray scene (no ray queries)");
                return;
            }
            assert_eq!(
                d.get("render.ray.mirror_pictures"),
                Some(1.0),
                "a traced picture"
            );
        }
        assert!(
            pixel(GOLD).y < 0 && pixel(CHROME).y < 0,
            "the balls are on screen"
        );
        let dir = std::env::temp_dir().join("engine-planar-reflections");
        let _ = std::fs::create_dir_all(&dir);
        let name = format!("metal-{what}.png");
        let path = dir.join(&name);
        capture_now(&mut app.world, &path).expect("screenshot");
        if let Ok(out) = std::env::var("ENGINE_SHOTS") {
            let _ = std::fs::copy(&path, std::path::Path::new(&out).join(&name));
        }
        let shot = image::open(&path).expect("read the shot").to_rgb8();
        // How much redder than blue a ball's reflection is round its middle (a mean of 7 x 7
        // pixels): what gold's own colour adds to the sky and the sun it reflects.
        let warmth = |p: DVec3| {
            let c = pixel(mirrored(p));
            let mut sum = [0.0f64; 3];
            let mut n = 0.0;
            for y in c.y - 3..=c.y + 3 {
                for x in c.x - 3..=c.x + 3 {
                    let q = shot.get_pixel(x as u32, y as u32).0;
                    for k in 0..3 {
                        sum[k] += f64::from(q[k]);
                    }
                    n += 1.0;
                }
            }
            (sum[0] - sum[2]) / n
        };
        let (gold, chrome) = (warmth(GOLD), warmth(CHROME));
        eprintln!(
            "{what}: red over blue in the gold ball's reflection {gold:.1}, in the chrome one's \
             {chrome:.1}"
        );
        assert!(
            gold > chrome + METAL_WARMER,
            "{what}: the gold ball's reflection is no yellower than the chrome one's: {gold:.1} \
             against {chrome:.1}"
        );
    }
}

/// How much redder than blue the gold ball's reflection is than the chrome one's, at least
/// (levels of 255; measured on lavapipe: 65 drawn and traced, 89 against 24).
const METAL_WARMER: f64 = 30.0;

/// With temporal anti-aliasing a traced picture holds still under a still camera, as a drawn
/// one does: each frame's rays go through another point of their texels (the frame's jitter),
/// and the picture's own history averages them (mirror_resolve.wgsl). Without it the picture
/// moved under the main view by a share of a texel each frame, and the reflections' outlines
/// changed from one frame to the next.
#[test]
fn a_traced_mirror_holds_still_under_temporal_anti_aliasing() {
    let mut app = ball_app(true);
    app.world.resource_mut::<RenderSettings>().anti_aliasing = AntiAliasing::Taa;
    app.update_n(40);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return;
    }
    let supported = app
        .world
        .resource::<Diagnostics>()
        .get("render.ray.supported");
    if supported != Some(1.0) {
        eprintln!("skipping: the GPU cannot hold the ray scene (no ray queries)");
        return;
    }
    let dir = std::env::temp_dir().join("engine-planar-reflections");
    let _ = std::fs::create_dir_all(&dir);
    // Frames in a row (each `update` places the mirror and traces its picture again).
    let shots: Vec<image::RgbImage> = (0..3)
        .map(|i| {
            let path = dir.join(format!("still-{i}.png"));
            app.world.resource_mut::<Screenshots>().request(&path);
            app.update();
            image::open(&path).expect("read the shot").to_rgb8()
        })
        .collect();
    assert_eq!(
        app.world
            .resource::<Diagnostics>()
            .get("render.ray.mirror_pictures"),
        Some(1.0),
        "a traced picture"
    );
    // The mirror's part of the screen (under the horizon, the reflections of the red and the
    // yellow ball in it): how far each pixel moved from one frame to the next. The balls'
    // outlines in the reflection are what moves. Measured on lavapipe, 400x300, the mirror's
    // picture at its default half resolution: with the picture's history, 9 and 23 levels at
    // most and 0 and 3 pixels past 16 (the real ball's outline above, the main view's own
    // history, 6 and 14 at most); without it, 41 and 76 at most and 10 and 19 pixels past 16.
    // The water's own shading keeps the average near 0.37 either way.
    let top = pixel(mirrored(RED)).y.min(pixel(mirrored(YELLOW)).y) as u32 - 20;
    for pair in shots.windows(2) {
        let (mut most, mut past, mut sum, mut n) = (0i32, 0u32, 0i64, 0i64);
        for y in top..WINDOW.y {
            for x in 0..WINDOW.x {
                let (a, b) = (pair[0].get_pixel(x, y).0, pair[1].get_pixel(x, y).0);
                let mut moved = 0;
                for c in 0..3 {
                    let d = (i32::from(a[c]) - i32::from(b[c])).abs();
                    moved = moved.max(d);
                    sum += i64::from(d);
                    n += 1;
                }
                most = most.max(moved);
                past += u32::from(moved > 16);
            }
        }
        let mean = sum as f64 / n as f64;
        eprintln!("a frame later: mean {mean:.3}, most {most}, past 16 {past} (rows {top}..)");
        assert!(mean < 0.6, "the reflection moved {mean:.3} on average");
        assert!(
            past <= 6,
            "{past} pixels of the reflection moved more than 16"
        );
        assert!(most <= 32, "a pixel of the reflection moved {most}");
    }
}

/// A white wall behind the mirror with a post's shadow on it, seen in the mirror: rendered with
/// the mirror lit by the main view's shadow maps (`mirror_shares_lighting`) or by its own.
/// Returns the shot and how many extra views borrowed the main view's maps.
fn render_wall(share: bool) -> Option<(image::RgbImage, f64)> {
    let mut app = wall_app(share);
    app.update_n(5);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let dir = std::env::temp_dir().join("engine-planar-reflections");
    let _ = std::fs::create_dir_all(&dir);
    let name = format!("wall-shared-{share}.png");
    let path = dir.join(&name);
    capture_now(&mut app.world, &path).expect("screenshot");
    if let Ok(out) = std::env::var("ENGINE_SHOTS") {
        let _ = std::fs::copy(&path, std::path::Path::new(&out).join(&name));
    }
    let shared = app
        .world
        .resource::<Diagnostics>()
        .get("render.extra_views_shared_lighting")
        .unwrap_or(0.0);
    Some((image::open(&path).expect("read the shot").to_rgb8(), shared))
}

/// The wall scene of [`render_wall`], not yet rendered.
fn wall_app(share: bool) -> App {
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
        s.mirror_shares_lighting = share;
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
        Transform::default().looking_at(SUN_TRAVEL, Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        PlanarReflection::new(Vec2::splat(SIZE)).with_resolution(1.0),
    ));
    w.spawn((
        Transform::from_translation(DVec3::new(0.0, 2.0, WALL_Z - 0.25)),
        MeshRenderer::new(
            Shape::Cuboid {
                size: [8.0, 4.0, 0.5],
            },
            StandardMaterial::color(0.8, 0.8, 0.8),
        ),
    ));
    w.spawn((
        Transform::from_translation(DVec3::new(0.0, 2.0, POST_Z)),
        MeshRenderer::new(
            Shape::Cuboid {
                size: [0.4, 4.0, 0.4],
            },
            StandardMaterial::color(0.8, 0.8, 0.8),
        ),
    ));
    app
}

#[test]
fn the_mirrors_numbers_go_to_zero_once_it_is_gone() {
    let mut app = wall_app(true);
    app.update_n(3);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return;
    }
    let get = |app: &App, key: &str| app.world.resource::<Diagnostics>().get(key).unwrap_or(-1.0);
    assert!(
        get(&app, "render.extra_views_ms") > 0.0,
        "the mirror's view"
    );
    let mirror = app
        .world
        .query_filtered::<Entity, With<PlanarReflection>>()
        .single(&app.world)
        .unwrap();
    app.world.despawn(mirror);
    app.update_n(3);
    for key in [
        "render.extra_views_ms",
        "render.extra_views_gpu_ms",
        "render.extra_views_shared_lighting",
    ] {
        assert_eq!(get(&app, key), 0.0, "{key} with no mirror");
    }
}

/// A mirror switched off and on again starts its view where it left it: same camera, same
/// picture targets, not built anew (a lake mirror moves from lake to lake that way).
#[test]
fn a_mirror_switched_off_and_on_keeps_its_view() {
    let mut app = wall_app(true);
    app.update_n(3);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return;
    }
    let mirror = app
        .world
        .query_filtered::<Entity, With<PlanarReflection>>()
        .single(&app.world)
        .unwrap();
    let picture = |app: &App| planar_reflection_picture(&app.world, mirror).cloned();
    let before = picture(&app).expect("a picture while on");
    app.world.get_mut::<PlanarReflection>(mirror).unwrap().on = false;
    app.update_n(3);
    assert!(picture(&app).is_none(), "no picture while off");
    let get = |app: &App, key: &str| app.world.resource::<Diagnostics>().get(key).unwrap_or(-1.0);
    assert_eq!(get(&app, "render.extra_views_ms"), 0.0, "no view while off");
    app.world.get_mut::<PlanarReflection>(mirror).unwrap().on = true;
    app.update_n(3);
    let after = picture(&app).expect("a picture once on again");
    assert_eq!(after.camera, before.camera, "the same view camera");
    assert!(
        after.depth == before.depth,
        "the same depth target, not built anew"
    );
}

/// A mirror whose picture the frame's rays trace has one every frame from its view camera,
/// which is placed but not drawn; drawn again, the view's first picture is not kept (a reader
/// sees none for a frame and starts over, as when a mirror comes on: the water lets a mirror's
/// first pictures settle), then its pictures come from the view's own targets, kept while it
/// was traced.
#[test]
fn a_traced_mirror_drawn_again_starts_over_with_its_own_view() {
    let mut app = ball_app(true);
    app.world.resource_mut::<RenderSettings>().trial_cuts_more &= !RAY_MIRROR_PICTURES;
    app.update_n(3);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return;
    }
    let get = |app: &App, key: &str| app.world.resource::<Diagnostics>().get(key).unwrap_or(-1.0);
    if get(&app, "render.ray.supported") != 1.0 {
        eprintln!("skipping: the GPU cannot hold the ray scene (no ray queries)");
        return;
    }
    let mirror = app
        .world
        .query_filtered::<Entity, With<PlanarReflection>>()
        .single(&app.world)
        .unwrap();
    let picture = |app: &App| planar_reflection_picture(&app.world, mirror).cloned();
    let drawn = picture(&app).expect("a drawn picture");
    assert_eq!(get(&app, "render.extra_views"), 1.0, "the mirror's view");
    let traced = |app: &mut App, on: bool| {
        let mut s = app.world.resource_mut::<RenderSettings>();
        if on {
            s.trial_cuts_more |= RAY_MIRROR_PICTURES;
        } else {
            s.trial_cuts_more &= !RAY_MIRROR_PICTURES;
        }
    };
    traced(&mut app, true);
    for frame in 0..3 {
        app.update();
        let p = picture(&app).unwrap_or_else(|| panic!("frame {frame}: no traced picture"));
        assert_eq!(
            p.camera, drawn.camera,
            "frame {frame}: the same view camera"
        );
        assert!(p.depth != drawn.depth, "frame {frame}: traced, not drawn");
        assert_eq!(
            get(&app, "render.ray.mirror_pictures"),
            1.0,
            "frame {frame}"
        );
        assert_eq!(
            get(&app, "render.extra_views"),
            0.0,
            "frame {frame}: no view drawn"
        );
    }
    traced(&mut app, false);
    app.update();
    assert!(
        picture(&app).is_none(),
        "the view's first picture after traced ones is not kept"
    );
    assert_eq!(get(&app, "render.extra_views"), 1.0, "the view is drawn");
    assert_eq!(get(&app, "render.ray.mirror_pictures"), 0.0);
    app.update();
    let again = picture(&app).expect("a drawn picture again");
    assert_eq!(again.camera, drawn.camera, "the same view camera");
    assert!(
        again.depth == drawn.depth,
        "the view's own targets, kept while it was traced"
    );
}

/// Where the sun's light travels: down, away from the camera and a little to the right.
const SUN_TRAVEL: DVec3 = DVec3::new(0.3, -0.5, -0.8);
/// The wall's front face and the post in front of it.
const WALL_Z: f64 = -8.0;
const POST_Z: f64 = -6.0;

/// Where the post's shadow falls on the wall's face at `height`.
fn post_shadow(height: f64) -> DVec3 {
    let t = (POST_Z - WALL_Z) / -SUN_TRAVEL.z;
    DVec3::new(SUN_TRAVEL.x * t, 0.0, WALL_Z) + DVec3::Y * height
}

#[test]
fn a_mirror_lit_by_the_main_views_shadow_maps_shows_the_same_shadows() {
    let (Some((own, own_shared)), Some((borrowed, shared))) =
        (render_wall(false), render_wall(true))
    else {
        return;
    };
    assert_eq!(own_shared, 0.0, "the mirror drew its own maps");
    assert_eq!(shared, 1.0, "the mirror borrowed the main view's maps");
    // In the reflection: the post's shadow on the wall (it reaches 2.75 m up the wall), and the
    // lit wall beside it.
    let shadow = mirrored(post_shadow(1.5));
    let lit = mirrored(DVec3::new(-2.5, 1.5, WALL_Z));
    let luma = |shot: &image::RgbImage, p: DVec3| {
        let q = pixel(p);
        assert!(
            q.cmpge(IVec2::ZERO).all() && q.cmplt(WINDOW.as_ivec2()).all(),
            "{p} is off screen at {q}"
        );
        let c = shot.get_pixel(q.x as u32, q.y as u32).0;
        (
            c[0] as f32 * 0.2126 + c[1] as f32 * 0.7152 + c[2] as f32 * 0.0722,
            c,
        )
    };
    for (name, shot) in [("own maps", &own), ("borrowed maps", &borrowed)] {
        let (dark, dark_rgb) = luma(shot, shadow);
        let (bright, bright_rgb) = luma(shot, lit);
        assert!(
            bright > dark + 40.0,
            "{name}: the reflected wall in the post's shadow {dark_rgb:?} is not darker than \
             the lit wall {bright_rgb:?}"
        );
    }
    for p in [shadow, lit] {
        let (a, a_rgb) = luma(&own, p);
        let (b, b_rgb) = luma(&borrowed, p);
        assert!(
            (a - b).abs() < 12.0,
            "the reflection at {p} differs: own maps {a_rgb:?}, borrowed {b_rgb:?}"
        );
    }
}

/// A feature's own fragment shader that traces the frame's ray scene, as the water's bodies do
/// (`trial_cut_more::RAY_WATER`): the mirror's square, reflecting along its normal with
/// `ray_reflection` (`ray_scene::water_reflection_lib`, the scene's group at 3) where the frame
/// hands it the group, else with the stub, which finds nothing (black).
const FLOOR_SHADER: &str = r#"
@group(1) @binding(0) var<uniform> floor_half: vec4<f32>; // half the square's side (m)

struct FloorOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) rel: vec3<f32>,
};

@vertex
fn vs_floor(@builtin(vertex_index) index: u32) -> FloorOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    let c = corners[index] * floor_half.x;
    let p = vec3<f32>(c.x, 0.0, c.y) - frame.camera_world.xyz;
    var out: FloorOut;
    out.clip = frame.view_proj * vec4<f32>(p, 1.0);
    out.rel = p;
    return out;
}

@fragment
fn fs_floor(in: FloorOut) -> @location(0) vec4<f32> {
    let f = ray_reflection(in.rel, reflect(normalize(in.rel), vec3<f32>(0.0, 1.0, 0.0)), 0.02);
    return vec4<f32>(f.rgb * f.a, 1.0);
}
"#;

/// Whether each draw of [`RayFloor`] was in an extra view, and whether it had the frame's ray
/// scene's group.
type FloorDraws = std::sync::Arc<std::sync::Mutex<Vec<(bool, bool)>>>;

/// Draws [`FLOOR_SHADER`] in the transparent phase, as the water draws its bodies: with the
/// tracing pipeline (`[frame, own, none, the ray scene's]`) where `FrameContext::ray_group` is
/// given, else with the stub's (`[frame, own]`).
struct RayFloor {
    pipelines: Option<(
        Option<wgpu::RenderPipeline>,
        wgpu::RenderPipeline,
        wgpu::BindGroup,
    )>,
    draws: FloorDraws,
}

impl RenderFeature for RayFloor {
    fn name(&self) -> &str {
        "test ray floor"
    }

    fn init(&mut self, gpu: &GpuShared) {
        let device = &gpu.device;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("test ray floor"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("test ray floor"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        gpu.queue.write_buffer(
            &buffer,
            0,
            bytemuck::cast_slice(&[SIZE * 0.5, 0.0, 0.0, 0.0]),
        );
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("test ray floor"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        let pipeline = |label: &str, lib: &str, layouts: &[Option<&wgpu::BindGroupLayout>]| {
            let shader = gpu.shader(label, &format!("{lib}\n{FLOOR_SHADER}"), true);
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: layouts,
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_floor"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: Default::default(),
                depth_stencil: Some(gpu.depth_state(true)),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_floor"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: gpu.formats.hdr,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let plain = pipeline(
            "test ray floor (stub)",
            engine_render::ray_scene::WATER_REFLECTION_STUB,
            &[Some(&gpu.frame_layout), Some(&layout)],
        );
        let rays = gpu.ray_group_layout().map(|ray| {
            pipeline(
                "test ray floor (rays)",
                &engine_render::ray_scene::water_reflection_lib(gpu.adapter_info.device_type),
                &[Some(&gpu.frame_layout), Some(&layout), None, Some(&ray)],
            )
        });
        self.pipelines = Some((rays, plain, group));
    }

    fn draw(&self, phase: RenderPhase, pass: &mut wgpu::RenderPass<'_>, ctx: &FrameContext) {
        if phase != RenderPhase::Transparent {
            return;
        }
        let Some((rays, plain, group)) = &self.pipelines else {
            return;
        };
        self.draws
            .lock()
            .unwrap()
            .push((ctx.view.extra_view, ctx.ray_group.is_some()));
        // The scene's group last, after every group below it (each bind of it costs the submit
        // a walk of its instances).
        match ctx.ray_group.zip(rays.as_ref()) {
            Some((ray, rays)) => {
                pass.set_pipeline(rays);
                pass.set_bind_group(1, group, &[]);
                pass.set_bind_group(3, ray, &[]);
            }
            None => {
                pass.set_pipeline(plain);
                pass.set_bind_group(1, group, &[]);
            }
        }
        pass.draw(0..6, 0..1);
    }
}

/// A feature's own shader traces the frame's ray scene as the water's bodies will
/// (`trial_cut_more::RAY_WATER`, step 12 part two): with the scene's group from
/// `FrameContext::ray_group` at 3, the mirror's square shows the red ball and the yellow one
/// above the top of the screen, lit as every pixel's reflection lights its hits, and nothing
/// under it. The group is handed over only in the main view: the mirror's own view, drawn as an
/// extra view here (`RAY_MIRROR_PICTURES` off), has none. Without the scene (its switch off) the
/// same square draws with the stub, which finds nothing.
#[test]
fn a_features_own_shader_reflects_through_the_frames_rays_what_is_above_it() {
    for rays in [true, false] {
        let draws = FloorDraws::default();
        let mut app = ball_app_with(
            false,
            RayFloor {
                pipelines: None,
                draws: draws.clone(),
            },
        );
        {
            let mut s = app.world.resource_mut::<RenderSettings>();
            s.probe_gi = true;
            s.gi_hardware_rays = true;
            s.gi_probe_spacing = 1.0;
            s.gi_probe_grid = [16, 8, 16];
            s.gi_probes_per_frame = 16;
            if rays {
                s.trial_cuts_more |= RAY_WORLD;
            }
        }
        app.update_n(5);
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("skipping: no GPU ({})", d.0);
            return;
        }
        let d = app.world.resource::<Diagnostics>();
        if d.get("render.ray.supported") != Some(1.0) {
            eprintln!("skipping: the GPU cannot hold the ray scene (no ray queries)");
            return;
        }
        assert_eq!(d.get("render.ray.on"), Some(if rays { 1.0 } else { 0.0 }));
        assert_eq!(d.get("render.extra_views"), Some(1.0), "the mirror's view");
        let name = format!("floor-{}.png", if rays { "rays" } else { "stub" });
        let dir = std::env::temp_dir().join("engine-planar-reflections");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(&name);
        capture_now(&mut app.world, &path).expect("screenshot");
        if let Ok(out) = std::env::var("ENGINE_SHOTS") {
            let _ = std::fs::copy(&path, std::path::Path::new(&out).join(&name));
        }
        let shot = image::open(&path).expect("read the shot").to_rgb8();
        let draws = draws.lock().unwrap().clone();
        let main: Vec<bool> = draws.iter().filter(|d| !d.0).map(|d| d.1).collect();
        let extra: Vec<bool> = draws.iter().filter(|d| d.0).map(|d| d.1).collect();
        eprintln!("rays {rays}: main view's draws {main:?}, the mirror view's {extra:?}");
        assert!(!extra.is_empty(), "the floor drew in the mirror's view");
        assert!(
            extra.iter().all(|&had| !had),
            "an extra view had the ray group"
        );
        assert!(main.len() >= 5, "the floor drew in every frame: {main:?}");
        if rays {
            // (From the frame the scene is made in, which may follow the first.)
            assert!(
                main[main.len() - 3..].iter().all(|&had| had),
                "a main view without the ray group: {main:?}"
            );
            assert_reflects_what_is_above(&shot, "the floor's own rays");
        } else {
            assert!(
                main.iter().all(|&had| !had),
                "a ray group without the scene"
            );
            let q = pixel(mirrored(YELLOW));
            let c = shot.get_pixel(q.x as u32, q.y as u32).0;
            assert!(c.iter().all(|&v| v < 24), "the stub found something: {c:?}");
        }
    }
}
