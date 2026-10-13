//! Heat haze bends the scene behind a fire without erasing what other features draw in the
//! transparent phase. A dark pane (standing in for water, clouds in front of the hills or rain)
//! veils the whole view; above the flames it must stay as dark as beside them.
//! Needs a GPU or lavapipe; passes with a note when neither is available.

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_fire::{ContainedFire, FirePlugin, FlameShape};
use engine_render::prelude::*;
use engine_render::wgpu;
use engine_render::{
    FrameContext, GpuShared, RenderAppExt, RenderDisabled, RenderFeature, RenderPhase, Renderer,
    capture_now,
};
use engine_world::{Combustible, WeatherState, WorldPlugin};
use image::RgbaImage;

const W: u32 = 320;
const H: u32 = 180;

/// A full-screen dark veil drawn early in the transparent phase, like water at -10.
#[derive(Default)]
struct Pane {
    pipeline: Option<wgpu::RenderPipeline>,
}

const PANE_SHADER: &str = r#"
@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.5, 1.0);
}

@fragment
fn fs() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 0.0, 0.0, 0.8);
}
"#;

impl RenderFeature for Pane {
    fn name(&self) -> &str {
        "test pane"
    }

    fn order(&self) -> i32 {
        -10
    }

    fn init(&mut self, gpu: &GpuShared) {
        let device = &gpu.device;
        let shader = gpu.shader("test pane", PANE_SHADER, true);
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("test pane"),
            bind_group_layouts: &[Some(&gpu.frame_layout)],
            immediate_size: 0,
        });
        let mut depth = gpu.depth_state(false);
        depth.depth_compare = Some(wgpu::CompareFunction::Always);
        self.pipeline = Some(
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("test pane"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: Default::default(),
                depth_stencil: Some(depth),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: gpu.formats.hdr,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            }),
        );
    }

    fn draw(&self, phase: RenderPhase, pass: &mut wgpu::RenderPass<'_>, _ctx: &FrameContext) {
        if phase != RenderPhase::Transparent {
            return;
        }
        if let Some(p) = &self.pipeline {
            pass.set_pipeline(p);
            pass.draw(0..3, 0..1);
        }
    }
}

fn app() -> App {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = W;
        w.height = H;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 30.0,
        });
    app.add_plugins((RenderPlugin, WorldPlugin, FirePlugin));
    app.add_render_feature(Pane::default());
    // Still air: the hot column stands straight up.
    {
        let mut weather = app.world.resource_mut::<WeatherState>();
        weather.wind = Vec3::ZERO;
        weather.gustiness = 0.0;
    }
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        s.tonemapper = Tonemapper::None;
        s.bloom_intensity = 0.0;
        s.vignette = 0.0;
        s.film_grain = 0.0;
        s.sharpness = 0.0;
        s.anti_aliasing = AntiAliasing::None;
    }
    app
}

/// Where world point `p` lands on screen, in pixels.
fn screen(app: &App, p: DVec3) -> (i32, i32) {
    let view = app
        .world
        .resource::<Renderer>()
        .last_view()
        .unwrap()
        .clone();
    let clip = view.unjittered_view_proj * (p - view.origin).as_vec3().extend(1.0);
    let ndc = clip.truncate() / clip.w;
    (
        ((ndc.x * 0.5 + 0.5) * W as f32) as i32,
        ((0.5 - ndc.y * 0.5) * H as f32) as i32,
    )
}

/// Mean brightness (sum of the channels) of the sky over the screen rectangle spanned by two
/// world points, leaving out flames and embers (redder than blue).
fn sky(img: &RgbaImage, a: (i32, i32), b: (i32, i32)) -> f32 {
    let (x0, x1) = (a.0.min(b.0).max(0), a.0.max(b.0).min(W as i32 - 1));
    let (y0, y1) = (a.1.min(b.1).max(0), a.1.max(b.1).min(H as i32 - 1));
    let mut sum = 0.0;
    let mut n = 0;
    for y in y0..=y1 {
        for x in x0..=x1 {
            let p = img.get_pixel(x as u32, y as u32);
            if p[0] > p[2] {
                continue;
            }
            sum += p[0] as f32 + p[1] as f32 + p[2] as f32;
            n += 1;
        }
    }
    sum / n.max(1) as f32
}

#[test]
fn haze_keeps_what_is_drawn_behind_the_fire() {
    let mut app = app();
    app.world.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [400.0, 400.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x4f5a34).with_roughness(0.95),
        ),
    ));
    // A fire that keeps burning, 1.5 m tall: its hot column shimmers up to about 3 m.
    app.world.spawn((
        Transform::default(),
        Combustible {
            fuel_kg: 20.0,
            moisture: 0.05,
            ignition_c: 300.0,
            temperature_c: 700.0,
            ..Default::default()
        },
        FlameShape {
            radius: 0.6,
            height: 1.5,
            offset: [0.0; 3],
        },
        ContainedFire { perpetual: true },
    ));
    app.world.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.3, -1.0, 0.6), Vec3::Y),
    ));
    // Low and looking up, so the sky is behind the hot column.
    app.world.spawn((
        Camera::default(),
        Transform::from_xyz(0.0, 0.8, -6.0).looking_at(DVec3::new(0.0, 2.2, 0.0), Vec3::Y),
    ));

    app.update_n(45);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return;
    }
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("fire_haze.png");
    capture_now(&mut app.world, &path).expect("screenshot");
    let img = image::open(&path).expect("png").to_rgba8();

    // Over the flame tips, where the air shimmers most, against the same band of sky beside it.
    let (lo, hi) = (1.3, 2.0);
    let column = sky(
        &img,
        screen(&app, DVec3::new(-0.4, lo, 0.0)),
        screen(&app, DVec3::new(0.4, hi, 0.0)),
    );
    let left = sky(
        &img,
        screen(&app, DVec3::new(-3.4, lo, 0.0)),
        screen(&app, DVec3::new(-2.6, hi, 0.0)),
    );
    let right = sky(
        &img,
        screen(&app, DVec3::new(2.6, lo, 0.0)),
        screen(&app, DVec3::new(3.4, hi, 0.0)),
    );
    let beside = 0.5 * (left + right);
    eprintln!(
        "above the fire {column:.1}, beside it {beside:.1} ({})",
        path.display()
    );
    assert!(beside > 30.0, "the veiled sky should still show: {beside}");
    assert!(
        column < beside * 1.25,
        "the haze erased the pane above the fire: {column:.1} vs {beside:.1} beside it"
    );
}
