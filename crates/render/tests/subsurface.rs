//! Subsurface scattering: a card of "skin" 30 cm from the camera, lit on its left half and in
//! shadow on its right, with a thin highlight (specular light, left in alpha) in the shadow.
//! When the feature marks it in the prepass, light spreads across the shadow's edge, red
//! farthest, while the highlight stays sharp and nothing outside the card changes. Needs a GPU
//! or lavapipe; passes with a note when neither is available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{FrameContext, GpuShared, RenderDisabled, capture_now, wgpu};
use image::RgbImage;

const W: u32 = 640;
const H: u32 = 360;

const SHADER: &str = r#"
struct CardOut {
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) local: vec2<f32>,
};

// A 20 cm card 30 cm in front of the camera (at the origin, looking down -z).
@vertex
fn vs_card(@builtin(vertex_index) i: u32) -> CardOut {
    let corners = array<vec2<f32>, 6>(
        vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(1.0, 1.0),
        vec2(-1.0, -1.0), vec2(1.0, 1.0), vec2(-1.0, 1.0),
    );
    let c = corners[i];
    var out: CardOut;
    out.clip = frame.view_proj * vec4<f32>(c.x * 0.1, c.y * 0.1, -0.3, 1.0);
    out.local = c * 0.1;
    return out;
}

@fragment
fn fs_card_prepass(in: CardOut) -> PrepassOutput {
    if (CARD_SCATTERS) {
        return prepass_output_subsurface(vec3<f32>(0.0, 0.0, 1.0), 0.5, in.clip, in.clip, 1.0);
    }
    return prepass_output(vec3<f32>(0.0, 0.0, 1.0), 0.5, in.clip, in.clip);
}

// Lit left of x = 0, dark right of it; a 3 mm highlight at x = 3 cm (its light in alpha).
@fragment
fn fs_card(in: CardOut) -> @location(0) vec4<f32> {
    let lit = select(0.0, 1.0, in.local.x < 0.0);
    let highlight = select(0.0, 1.0, abs(in.local.x - 0.03) < 0.0015);
    let diffuse = pre_expose(vec3<f32>(lit * 3000.0));
    let specular = pre_expose(vec3<f32>(highlight * 3000.0)).x;
    return vec4<f32>(diffuse + vec3<f32>(specular), specular);
}
"#;

struct SkinCard {
    scatters: bool,
    pipelines: Option<(wgpu::RenderPipeline, wgpu::RenderPipeline)>,
}

impl RenderFeature for SkinCard {
    fn name(&self) -> &str {
        "skin card"
    }

    fn init(&mut self, gpu: &GpuShared) {
        let source = SHADER.replace("CARD_SCATTERS", &self.scatters.to_string());
        let module = gpu.shader("skin card", &source, true);
        let layout = gpu
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("skin card"),
                bind_group_layouts: &[Some(&gpu.frame_layout)],
                immediate_size: 0,
            });
        let pipeline = |fs: &str, targets: &[Option<wgpu::ColorTargetState>], write: bool| {
            gpu.device
                .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("skin card"),
                    layout: Some(&layout),
                    vertex: wgpu::VertexState {
                        module: &module,
                        entry_point: Some("vs_card"),
                        compilation_options: Default::default(),
                        buffers: &[],
                    },
                    primitive: Default::default(),
                    depth_stencil: Some(gpu.depth_state(write)),
                    multisample: Default::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &module,
                        entry_point: Some(fs),
                        compilation_options: Default::default(),
                        targets,
                    }),
                    multiview_mask: None,
                    cache: None,
                })
        };
        let prepass = pipeline("fs_card_prepass", &gpu.prepass_targets(), true);
        let scene = pipeline(
            "fs_card",
            &[Some(wgpu::ColorTargetState {
                format: gpu.formats.hdr,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            false,
        );
        self.pipelines = Some((prepass, scene));
    }

    fn draw_prepass(&self, pass: &mut wgpu::RenderPass<'_>, _ctx: &FrameContext) {
        if let Some((prepass, _)) = &self.pipelines {
            pass.set_pipeline(prepass);
            pass.draw(0..6, 0..1);
        }
    }

    fn draw(&self, phase: RenderPhase, pass: &mut wgpu::RenderPass<'_>, _ctx: &FrameContext) {
        if phase != RenderPhase::Opaque {
            return;
        }
        if let Some((_, scene)) = &self.pipelines {
            pass.set_pipeline(scene);
            pass.draw(0..6, 0..1);
        }
    }

    fn needs_subsurface(&self) -> bool {
        self.scatters
    }
}

fn render(scatters: bool, shot: &str) -> Option<RgbImage> {
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
        s.anti_aliasing = AntiAliasing::None;
        s.film_grain = 0.0;
        s.bloom_intensity = 0.0;
        s.vignette = 0.0;
    }
    app.add_render_feature(SkinCard {
        scatters,
        pipelines: None,
    });
    app.world.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 12.0 },
            ..Camera::perspective(60.0)
        },
        Transform::default(),
    ));
    app.update_n(3);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let dir = std::env::temp_dir().join("engine-subsurface");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(shot);
    capture_now(&mut app.world, &path).expect("screenshot");
    if let Ok(out) = std::env::var("ENGINE_SHOTS") {
        let _ = std::fs::copy(&path, std::path::Path::new(&out).join(shot));
    }
    Some(image::open(&path).expect("read the shot").to_rgb8())
}

/// The card's x (meters) → the shot's column, along its middle row.
fn column(x: f32) -> u32 {
    // 60° vertical field of view, the card 0.3 m away.
    let px_per_m = (H as f32 / 2.0) / (30f32.to_radians().tan() * 0.3);
    (W as f32 / 2.0 + x * px_per_m).round() as u32
}

#[test]
fn light_spreads_under_skin_red_farthest_and_highlights_stay_sharp() {
    let Some(plain) = render(false, "skin_plain.png") else {
        return;
    };
    let Some(skin) = render(true, "skin_scattered.png") else {
        return;
    };
    let y = H / 2;
    let at = |img: &RgbImage, x: f32| img.get_pixel(column(x), y).0;
    // 2 mm into the shadow: dark without scattering, reddish light with it.
    let p = at(&plain, 0.002);
    let s = at(&skin, 0.002);
    eprintln!("2 mm into the shadow: plain {p:?}, scattered {s:?}");
    assert!(
        p.iter().all(|&c| c < 8),
        "the plain card is dark there: {p:?}"
    );
    assert!(s[0] > 30, "light spreads into the shadow: {s:?}");
    assert!(
        s[0] > s[1] && s[1] >= s[2],
        "red reaches farthest, blue least: {s:?}"
    );
    // The lit side loses a little light to the shadow at its edge.
    let (pl, sl) = (at(&plain, -0.002), at(&skin, -0.002));
    assert!(
        sl[2] < pl[2],
        "the lit edge gives light away: {pl:?} -> {sl:?}"
    );
    // The highlight keeps its peak (specular doesn't scatter).
    let (ph, sh) = (at(&plain, 0.03), at(&skin, 0.03));
    eprintln!("highlight: plain {ph:?}, scattered {sh:?}");
    assert!(
        sh[2] as f32 >= 0.9 * ph[2] as f32,
        "the highlight stays: {ph:?} -> {sh:?}"
    );
    // Off the card (the sky above it) nothing changes.
    for (x, yy) in [(5, 5), (W - 5, H - 5), (W / 2, 10)] {
        assert_eq!(
            plain.get_pixel(x, yy),
            skin.get_pixel(x, yy),
            "off the card at ({x}, {yy})"
        );
    }
}
