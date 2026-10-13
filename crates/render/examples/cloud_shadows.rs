//! Cloud shadows from a render feature: a reference for modules that draw clouds. The feature
//! asks for the cloud shadow map (`RenderFeature::cloud_shadows`) and, in `compute`, marches
//! up from every texel's point on the cloud base toward the sun through world-anchored noise
//! clouds drifting with the wind. Saves the same landscape without and with them.
//!
//! ```sh
//! cargo run -p engine_render --example cloud_shadows -- <out-dir> [prefix]
//! ```

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{
    CLOUD_SHADOW_FORMAT, CloudShadowRequest, FrameContext, GpuShared, capture_now, wgpu,
};
use engine_world::WeatherState;

/// Cumulus between 1200 and 2000 m.
const BASE: f64 = 1200.0;
const TOP: f64 = 2000.0;

/// Fragment shader: the sunlight reaching each texel's point on the cloud base.
const CLOUDS: &str = r#"
@vertex
fn vs_clouds(@builtin(vertex_index) i: u32) -> FullscreenOut {
    return fullscreen_vertex(i, 0.0);
}

// Cloud density (extinction per meter) at camera-relative `p`: noise anchored to the world,
// drifting with the wind.
fn example_cloud_density(p: vec3<f32>) -> f32 {
    let w = (p + frame.camera_world.xyz).xz - frame.wind.xz * frame.time.x;
    let cover = smoothstep(0.48, 0.62, fbm2(w / 800.0, 5));
    return cover * 0.006;
}

@fragment
fn fs_clouds(in: FullscreenOut) -> @location(0) vec4<f32> {
    // This texel's point on the cloud base (camera-relative), then up toward the sun through
    // the layer.
    let base = cloud_shadow_plane_point(in.uv);
    let l = frame.sun_dir.xyz;
    let thickness = frame.cloud_shadow_params.z;
    let run = thickness / max(l.y, 0.05);
    var optical = 0.0;
    for (var i = 0; i < 8; i++) {
        let p = base + l * (run * (f32(i) + 0.5) / 8.0);
        optical += example_cloud_density(p) * run / 8.0;
    }
    return vec4<f32>(exp(-optical), 1.0, 1.0, 1.0);
}
"#;

#[derive(Default)]
struct Clouds {
    pipeline: Option<wgpu::RenderPipeline>,
}

impl RenderFeature for Clouds {
    fn name(&self) -> &str {
        "example clouds"
    }

    fn init(&mut self, gpu: &GpuShared) {
        let module = gpu.shader("example clouds", CLOUDS, true);
        let layout = gpu
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("example clouds"),
                bind_group_layouts: &[Some(&gpu.frame_layout)],
                immediate_size: 0,
            });
        self.pipeline = Some(
            gpu.device
                .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("example clouds"),
                    layout: Some(&layout),
                    vertex: wgpu::VertexState {
                        module: &module,
                        entry_point: Some("vs_clouds"),
                        compilation_options: Default::default(),
                        buffers: &[],
                    },
                    primitive: Default::default(),
                    depth_stencil: None,
                    multisample: Default::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &module,
                        entry_point: Some("fs_clouds"),
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
        // 32 m texels over 16 km.
        Some(CloudShadowRequest {
            size: 512,
            extent: 16_384.0,
            base_height: BASE,
            top_height: TOP,
            outside: 0.7,
        })
    }

    fn compute(&mut self, encoder: &mut wgpu::CommandEncoder, ctx: &FrameContext) {
        let (Some(target), Some(pipeline)) = (ctx.cloud_shadows, &self.pipeline) else {
            return;
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("example clouds"),
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
        pass.set_bind_group(0, ctx.frame_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

fn landscape(clouds: bool) -> App {
    engine_core::log::init("warn");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = 960;
        w.height = 540;
    }
    app.add_plugins(RenderPlugin);
    app.world.insert_resource(WeatherState {
        wind: Vec3::new(6.0, 0.0, 2.0),
        ..Default::default()
    });
    {
        let mut fog = app.world.resource_mut::<FogSettings>();
        fog.density = 0.00025;
        fog.height_falloff = 0.003;
    }
    if clouds {
        app.add_render_feature(Clouds::default());
    }
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.3 },
            ..Camera::perspective(55.0)
        },
        Transform::from_xyz(0.0, 70.0, 300.0).looking_at(DVec3::new(0.0, 0.0, -700.0), Vec3::Y),
    ));
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(-0.45, -0.75, -0.5), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [12_000.0, 12_000.0],
                subdivisions: 8,
            },
            StandardMaterial::color(0.24, 0.3, 0.14)
                .with_roughness(0.95)
                .with_detail(1.0, 6.0)
                .with_detail_in_world(),
        ),
    ));
    // Buildings and boulders for scale.
    let stone = StandardMaterial::color(0.55, 0.52, 0.48).with_roughness(0.8);
    for i in 0..14 {
        let f = i as f64;
        let x = (f * 137.0) % 900.0 - 450.0;
        let z = -100.0 - (f * 211.0) % 1400.0;
        let h = 10.0 + (f * 7.0) % 30.0;
        w.spawn((
            Transform::from_xyz(x, h * 0.5, z),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [24.0, h as f32, 24.0],
                },
                stone.clone(),
            ),
        ));
    }
    app
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = std::path::PathBuf::from(
        args.first()
            .cloned()
            .unwrap_or_else(|| "cloud_shadows".into()),
    );
    let prefix = args.get(1).cloned().unwrap_or_default();
    std::fs::create_dir_all(&out).expect("output directory");
    for (clouds, name) in [(false, "before"), (true, "after")] {
        let mut app = landscape(clouds);
        app.update_n(8);
        let path = out.join(format!("{prefix}cloud_shadows_{name}.png"));
        capture_now(&mut app.world, &path).expect("screenshot");
        println!("{}", path.display());
    }
}
