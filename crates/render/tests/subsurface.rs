//! Subsurface scattering: a card of "skin" 30 cm from the camera, lit on its left half and in
//! shadow on its right, with a thin highlight (specular light, left in alpha) in the shadow.
//! When the feature marks it in the prepass, light spreads across the shadow's edge, red
//! farthest, while the highlight stays sharp and nothing outside the card changes. A textured
//! card (fair skin with a freckle) shows what post-scatter color keeps: the freckle stays sharp
//! where blurring textured light would wash it out. Needs a GPU or lavapipe; passes with a note
//! when neither is available.

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

// White, or fair skin with a 4 mm freckle at x = -5 cm.
fn skin_albedo(local: vec2<f32>) -> vec3<f32> {
    if (CARD_ALBEDO == 0u) {
        return vec3<f32>(1.0);
    }
    let freckle = abs(local.x + 0.05) < 0.002;
    return select(vec3<f32>(0.85, 0.65, 0.55), vec3<f32>(0.35, 0.2, 0.15), freckle);
}

// Lit left of x = 0, dark right of it; a 3 mm highlight at x = 3 cm (its light in alpha). With
// post-scatter color (CARD_ALBEDO 2) the light is untextured and the albedo is drawn below.
@fragment
fn fs_card(in: CardOut) -> @location(0) vec4<f32> {
    let lit = select(0.0, 1.0, in.local.x < 0.0);
    let highlight = select(0.0, 1.0, abs(in.local.x - 0.03) < 0.0015);
    let albedo = select(skin_albedo(in.local), vec3<f32>(1.0), CARD_ALBEDO == 2u);
    let diffuse = pre_expose(albedo * lit * 3000.0);
    let specular = pre_expose(vec3<f32>(highlight * 3000.0)).x;
    return vec4<f32>(diffuse + vec3<f32>(specular), specular);
}

@fragment
fn fs_card_color(in: CardOut) -> @location(0) vec4<f32> {
    return vec4<f32>(skin_albedo(in.local), 1.0);
}
"#;

/// How the card is colored.
#[derive(Clone, Copy, PartialEq)]
enum Albedo {
    White,
    /// Textured light, blurred with it (pre-scatter).
    Lit,
    /// Untextured light; the color multiplied in after the blur (post-scatter).
    Post,
}

struct SkinCard {
    scatters: bool,
    albedo: Albedo,
    pipelines: Option<[wgpu::RenderPipeline; 3]>,
}

impl RenderFeature for SkinCard {
    fn name(&self) -> &str {
        "skin card"
    }

    fn init(&mut self, gpu: &GpuShared) {
        let source = SHADER
            .replace("CARD_SCATTERS", &self.scatters.to_string())
            .replace("CARD_ALBEDO", &format!("{}u", self.albedo as u32));
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
        let color = pipeline(
            "fs_card_color",
            &[Some(gpu.subsurface_color_target())],
            false,
        );
        self.pipelines = Some([prepass, scene, color]);
    }

    fn draw_prepass(&self, pass: &mut wgpu::RenderPass<'_>, _ctx: &FrameContext) {
        if let Some([prepass, ..]) = &self.pipelines {
            pass.set_pipeline(prepass);
            pass.draw(0..6, 0..1);
        }
    }

    fn draw(&self, phase: RenderPhase, pass: &mut wgpu::RenderPass<'_>, _ctx: &FrameContext) {
        if phase != RenderPhase::Opaque {
            return;
        }
        if let Some([_, scene, _]) = &self.pipelines {
            pass.set_pipeline(scene);
            pass.draw(0..6, 0..1);
        }
    }

    fn needs_subsurface(&self) -> bool {
        self.scatters
    }

    fn draw_subsurface_color(&self, pass: &mut wgpu::RenderPass<'_>, _ctx: &FrameContext) {
        if self.albedo == Albedo::Post
            && let Some([_, _, color]) = &self.pipelines
        {
            pass.set_pipeline(color);
            pass.draw(0..6, 0..1);
        }
    }
}

fn render(scatters: bool, albedo: Albedo, shot: &str) -> Option<RgbImage> {
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
        albedo,
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
    let Some(plain) = render(false, Albedo::White, "skin_plain.png") else {
        return;
    };
    let Some(skin) = render(true, Albedo::White, "skin_scattered.png") else {
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

#[test]
fn post_scatter_color_keeps_freckles_sharp_and_skin_its_tint() {
    let Some(plain) = render(false, Albedo::Lit, "freckle_plain.png") else {
        return;
    };
    let Some(pre) = render(true, Albedo::Lit, "freckle_pre_scatter.png") else {
        return;
    };
    let Some(post) = render(true, Albedo::Post, "freckle_post_scatter.png") else {
        return;
    };
    let y = H / 2;
    let at = |img: &RgbImage, x: f32| img.get_pixel(column(x), y).0;
    // How much darker the freckle is than the skin 1 cm beside it (green, which scatters
    // least of what is visible there, still shows the blur).
    let contrast = |img: &RgbImage| {
        let (skin, freckle) = (at(img, -0.06), at(img, -0.05));
        (0..3)
            .map(|c| skin[c] as f32 - freckle[c] as f32)
            .sum::<f32>()
    };
    let (cp, cpre, cpost) = (contrast(&plain), contrast(&pre), contrast(&post));
    eprintln!("freckle contrast: unscattered {cp}, pre-scatter {cpre}, post-scatter {cpost}");
    assert!(
        cpre < 0.85 * cp,
        "blurring textured light washes the freckle out: {cp} -> {cpre}"
    );
    assert!(
        cpost > 0.95 * cp,
        "post-scatter color keeps it: {cp} -> {cpost}"
    );
    // Away from the freckle and the shadow, the skin keeps its color.
    let (sp, spost) = (at(&plain, -0.075), at(&post, -0.075));
    for c in 0..3 {
        assert!(
            (sp[c] as i32 - spost[c] as i32).abs() <= 4,
            "the skin keeps its tint: {sp:?} -> {spost:?}"
        );
    }
    // Off the card nothing changes.
    for (x, yy) in [(5, 5), (W - 5, H - 5), (W / 2, 10)] {
        assert_eq!(
            plain.get_pixel(x, yy),
            post.get_pixel(x, yy),
            "off the card at ({x}, {yy})"
        );
    }
}
