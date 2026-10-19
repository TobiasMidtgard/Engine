//! Cloud shadows: a render feature draws the top-down cloud shadow map in `compute` (here 0.2
//! of the sunlight, or none, wherever world x < 0), and the sunlight on the ground and in the
//! fog there drops to that share while the rest of the image stays as it was. A map nobody
//! draws changes nothing, and the map stays anchored to the world when the camera moves. Needs
//! a GPU or lavapipe; passes with a note when neither is available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{
    CLOUD_SHADOW_FORMAT, CloudShadowRequest, FrameContext, GpuShared, RenderDisabled, Renderer,
    capture_now, wgpu,
};
use image::RgbaImage;

const W: u32 = 320;
const H: u32 = 180;

/// The cloud base the test map lies on, meters.
const BASE: f64 = 30.0;

/// A stand-in for the weather module: asks for a map 256 m across of `size`² texels and draws
/// `value` into it wherever the cloud base lies at world x < 0 (`None`: asks, but draws
/// nothing).
struct HalfCloud {
    value: Option<f32>,
    size: u32,
    pipeline: Option<wgpu::RenderPipeline>,
}

impl HalfCloud {
    /// 1 m texels: the shadow's edge blends over a meter.
    fn new(value: Option<f32>) -> Self {
        Self::with_size(value, 256)
    }

    fn with_size(value: Option<f32>, size: u32) -> Self {
        Self {
            value,
            size,
            pipeline: None,
        }
    }
}

impl RenderFeature for HalfCloud {
    fn name(&self) -> &str {
        "half cloud"
    }

    fn init(&mut self, gpu: &GpuShared) {
        let Some(value) = self.value else {
            return;
        };
        let source = format!(
            r#"
const HALF_CLOUD_VALUE: f32 = {value:?};

@vertex
fn vs_half_cloud(@builtin(vertex_index) i: u32) -> FullscreenOut {{
    return fullscreen_vertex(i, 0.0);
}}

@fragment
fn fs_half_cloud(in: FullscreenOut) -> @location(0) vec4<f32> {{
    // World x of this texel's center on the cloud base.
    let x = cloud_shadow_plane_point(in.uv).x + frame.camera_world.x;
    return vec4<f32>(select(1.0, HALF_CLOUD_VALUE, x < 0.0), 1.0, 1.0, 1.0);
}}
"#
        );
        let module = gpu.shader("half cloud", &source, true);
        let layout = gpu
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("half cloud"),
                bind_group_layouts: &[Some(&gpu.frame_layout)],
                immediate_size: 0,
            });
        self.pipeline = Some(
            gpu.device
                .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("half cloud"),
                    layout: Some(&layout),
                    vertex: wgpu::VertexState {
                        module: &module,
                        entry_point: Some("vs_half_cloud"),
                        compilation_options: Default::default(),
                        buffers: &[],
                    },
                    primitive: Default::default(),
                    depth_stencil: None,
                    multisample: Default::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &module,
                        entry_point: Some("fs_half_cloud"),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: CLOUD_SHADOW_FORMAT,
                            blend: None,
                            write_mask: wgpu::ColorWrites::RED,
                        })],
                    }),
                    multiview_mask: None,
                    cache: None,
                }),
        );
    }

    fn cloud_shadows(&self) -> Option<CloudShadowRequest> {
        Some(CloudShadowRequest {
            size: self.size,
            extent: 256.0,
            base_height: BASE,
            top_height: BASE + 20.0,
            outside: 1.0,
        })
    }

    fn compute(&mut self, encoder: &mut wgpu::CommandEncoder, ctx: &FrameContext) {
        let (Some(target), Some(pipeline)) = (ctx.cloud_shadows, &self.pipeline) else {
            return;
        };
        assert_eq!(target.area.size, self.size);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("half cloud"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(pipeline);
        // The frame group holds a stand-in for the map during `compute`, so binding it here
        // doesn't clash with drawing into the map.
        pass.set_bind_group(0, ctx.frame_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// Flat ground of albedo `albedo` seen from 30 m up, the sun ahead (27° up, in the y-z
/// plane, so the shadow's edge on the ground stays at x = 0), with `cloud` drawing the map.
fn ground_app(
    albedo: f32,
    camera_x: f64,
    cloud: Option<HalfCloud>,
    configure: impl FnOnce(&mut RenderSettings, &mut FogSettings),
) -> App {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = W;
        w.height = H;
    }
    app.add_plugins(RenderPlugin);
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        s.tonemapper = Tonemapper::None;
        s.bloom_intensity = 0.0;
        s.vignette = 0.0;
        s.film_grain = 0.0;
        s.saturation = 1.0;
        s.sharpness = 0.0;
        s.anti_aliasing = AntiAliasing::None;
        s.screen_space_gi = false;
        s.screen_space_reflections = false;
        s.volumetric_fog = false;
    }
    app.world.resource_mut::<FogSettings>().density = 0.0;
    world_scope(&mut app, configure);
    if let Some(cloud) = cloud {
        app.add_render_feature(cloud);
    }
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.0 },
            ..Camera::perspective(60.0)
        },
        Transform::from_xyz(camera_x, 30.0, 45.0)
            .looking_at(DVec3::new(camera_x, 0.0, 0.0), Vec3::Y),
    ));
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.0, -0.5, 1.0), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [400.0, 400.0],
                subdivisions: 1,
            },
            StandardMaterial::color(albedo, albedo, albedo).with_roughness(1.0),
        ),
    ));
    app
}

fn world_scope(app: &mut App, configure: impl FnOnce(&mut RenderSettings, &mut FogSettings)) {
    let mut fog = *app.world.resource::<FogSettings>();
    let mut settings = app.world.resource::<RenderSettings>().clone();
    configure(&mut settings, &mut fog);
    app.world.insert_resource(fog);
    app.world.insert_resource(settings);
}

fn render(app: &mut App, name: &str) -> Option<RgbaImage> {
    app.update_n(4);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    Some(image::open(&path).expect("png").to_rgba8())
}

fn linear(v: u8) -> f32 {
    let v = v as f32 / 255.0;
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear luminance of pixel (`x`, `y`), clamped to the image.
fn pixel_luminance(img: &RgbaImage, x: i32, y: i32) -> f32 {
    let px = img.get_pixel(
        x.clamp(0, W as i32 - 1) as u32,
        y.clamp(0, H as i32 - 1) as u32,
    );
    0.2126 * linear(px[0]) + 0.7152 * linear(px[1]) + 0.0722 * linear(px[2])
}

/// Mean linear luminance of a 7x7 pixel block centered exactly on where world point `p` lands
/// on screen (bilinear between pixels, so moving the camera doesn't shift it by a fraction of
/// a pixel).
fn luminance_at(app: &App, img: &RgbaImage, p: DVec3) -> f32 {
    let view = app
        .world
        .resource::<Renderer>()
        .last_view()
        .unwrap()
        .clone();
    let clip = view.unjittered_view_proj * (p - view.origin).as_vec3().extend(1.0);
    let ndc = clip.truncate() / clip.w;
    // Pixel centers sit at .5.
    let fx = (ndc.x * 0.5 + 0.5) * W as f32 - 0.5;
    let fy = (0.5 - ndc.y * 0.5) * H as f32 - 0.5;
    let (x0, y0) = (fx.floor() as i32, fy.floor() as i32);
    let (wx, wy) = (fx - fx.floor(), fy - fy.floor());
    let mut sum = 0.0;
    for dy in -3..=3 {
        for dx in -3..=3 {
            let (x, y) = (x0 + dx, y0 + dy);
            let top = pixel_luminance(img, x, y) * (1.0 - wx) + pixel_luminance(img, x + 1, y) * wx;
            let bottom = pixel_luminance(img, x, y + 1) * (1.0 - wx)
                + pixel_luminance(img, x + 1, y + 1) * wx;
            sum += top * (1.0 - wy) + bottom * wy;
        }
    }
    sum / 49.0
}

/// Ground under the cloud (x < 0) and in the sun (x > 0), well away from the edge.
const SHADED: DVec3 = DVec3::new(-14.0, 0.0, 0.0);
const SUNLIT: DVec3 = DVec3::new(14.0, 0.0, 0.0);

#[test]
fn ground_under_the_cloud_gets_its_share_of_the_sun() {
    let mut images = Vec::new();
    for (name, cloud) in [
        ("none", None),
        ("half_0.2", Some(0.2)),
        ("half_0", Some(0.0)),
    ] {
        let mut app = ground_app(0.5, 0.0, cloud.map(|v| HalfCloud::new(Some(v))), |_, _| {});
        let Some(img) = render(&mut app, &format!("cloud_shadow_ground_{name}")) else {
            return;
        };
        images.push((
            luminance_at(&app, &img, SHADED),
            luminance_at(&app, &img, SUNLIT),
        ));
    }
    let [(full, full_lit), (fifth, fifth_lit), (none, none_lit)] = images[..] else {
        unreachable!()
    };
    // Sky light is the same in all three, so the sun's share is what the map changes.
    let share = (fifth - none) / (full - none);
    eprintln!(
        "shaded ground: {full:.4} without clouds, {fifth:.4} at 0.2, {none:.4} at 0 \
         (sun share {share:.3}); sunlit ground: {full_lit:.4}, {fifth_lit:.4}, {none_lit:.4}"
    );
    assert!(
        full > none * 1.5,
        "the sun should light the ground well beyond the sky ({full} vs {none})"
    );
    assert!(
        (share - 0.2).abs() < 0.04,
        "the ground under a 0.2 cloud gets {share:.3} of the sun"
    );
    for lit in [fifth_lit, none_lit] {
        assert!(
            (lit - full_lit).abs() <= full_lit * 0.01 + 0.002,
            "the ground outside the cloud changed: {lit} vs {full_lit}"
        );
    }
}

#[test]
fn a_cloud_map_nobody_draws_changes_nothing() {
    let mut without = ground_app(0.5, 0.0, None, |_, _| {});
    // Asks for the map (so shading reads it) but leaves it as cleared.
    let mut cleared = ground_app(0.5, 0.0, Some(HalfCloud::new(None)), |_, _| {});
    let (Some(a), Some(b)) = (
        render(&mut without, "cloud_shadow_none"),
        render(&mut cleared, "cloud_shadow_cleared"),
    ) else {
        return;
    };
    // The images differ only by the output dither (it changes with the clock): at most two
    // levels in a pixel and nothing on average.
    let (mut worst, mut sum) = (0, [0i64; 3]);
    for (p, q) in a.pixels().zip(b.pixels()) {
        for c in 0..3 {
            let d = q[c] as i32 - p[c] as i32;
            worst = worst.max(d.abs());
            sum[c] += d as i64;
        }
    }
    let mean = sum.map(|s| s as f64 / (W * H) as f64);
    eprintln!("cleared map vs none: worst {worst} levels, mean {mean:?}");
    assert!(
        worst <= 2 && mean.iter().all(|m| m.abs() < 0.05),
        "a cleared cloud shadow map changed the image (worst {worst} levels, mean {mean:?})"
    );
}

#[test]
fn cloud_shadows_stay_put_when_the_camera_moves() {
    // With 16 m texels the shadow's edge on the ground blends over 16 m around x = 0. Moved by
    // a fraction of a texel, a camera with a map that followed it would see the blend shift.
    let mut values = Vec::new();
    for camera_x in [0.0, 6.6] {
        let cloud = HalfCloud::with_size(Some(0.0), 16);
        let mut app = ground_app(0.5, camera_x, Some(cloud), |_, _| {});
        let Some(img) = render(&mut app, &format!("cloud_shadow_moved_{camera_x}")) else {
            return;
        };
        values.push([-4.0, 0.0, 4.0].map(|x| luminance_at(&app, &img, DVec3::new(x, 0.0, 0.0))));
    }
    eprintln!("ground across the edge: {values:?}");
    for (a, b) in values[0].iter().zip(&values[1]) {
        assert!(
            (a - b).abs() <= a.max(*b) * 0.03 + 0.002,
            "the cloud shadow moved with the camera: {values:?}"
        );
    }
    // And the edge is a blend, not a step (the samples sit on the ramp).
    assert!(values[0][0] < values[0][2] * 0.95, "{values:?}");
}

#[test]
fn fog_under_the_cloud_is_not_sunlit() {
    // Black ground in thick fog: what the camera sees is the fog's in-scattered light alone.
    for volumetric in [true, false] {
        let mut results = Vec::new();
        for cloud in [None, Some(0.0)] {
            let mut app = ground_app(0.0, 0.0, cloud.map(|v| HalfCloud::new(Some(v))), |s, f| {
                s.volumetric_fog = volumetric;
                f.density = 0.02;
                f.height_falloff = 0.01;
                f.base_height = 0.0;
            });
            let name = format!("cloud_shadow_fog_{volumetric}_{}", cloud.is_some());
            let Some(img) = render(&mut app, &name) else {
                return;
            };
            // Far from the edge, so the rays there run through shaded or sunlit air all the way
            // from the camera (which stands over the edge).
            results.push((
                luminance_at(&app, &img, DVec3::new(-25.0, 0.0, 0.0)),
                luminance_at(&app, &img, DVec3::new(25.0, 0.0, 0.0)),
            ));
        }
        let [(clear_shaded, clear_lit), (cloud_shaded, cloud_lit)] = results[..] else {
            unreachable!()
        };
        eprintln!(
            "fog (volumetric {volumetric}): without clouds {clear_shaded:.4} / {clear_lit:.4}, \
             with {cloud_shaded:.4} / {cloud_lit:.4}"
        );
        assert!(
            cloud_shaded < clear_shaded * 0.75,
            "volumetric {volumetric}: fog under the cloud still glows with sunlight \
             ({cloud_shaded} vs {clear_shaded})"
        );
        // The fog in the sun keeps its sunlight; it only loses some of the light bounced up
        // from the ground below the camera, half of which is now in the cloud's shadow (the
        // environment map's ground: about 5% of what this fog scatters).
        assert!(
            cloud_lit < clear_lit * 1.01 && cloud_lit > clear_lit * 0.9,
            "volumetric {volumetric}: fog in the sun changed ({cloud_lit} vs {clear_lit})"
        );
    }
}
