//! Several cameras in one frame (`ExtraView`): a forest clearing seen by one camera, then split
//! screen, a picture-in-picture map view, and a screen in the clearing showing a camera across
//! the pond.
//!
//! ```sh
//! cargo run -p engine_render --example multi_view -- <out-dir> [prefix]
//! ```

use engine_assets::Assets;
use engine_core::prelude::*;
use engine_render::capture_now;
use engine_render::prelude::*;
use engine_world::{QualityPreset, Scalability};

const W: u32 = 960;
const H: u32 = 540;

#[derive(Clone, Copy, PartialEq)]
enum Layout {
    One,
    Split,
    Inset,
    Monitor,
}

fn camera(at: DVec3, target: DVec3) -> (Camera, Transform) {
    (
        Camera {
            exposure: Exposure::Manual { ev100: 13.5 },
            ..Camera::perspective(60.0)
        },
        Transform::from_translation(at).looking_at(target, Vec3::Y),
    )
}

fn clearing(layout: Layout) -> App {
    engine_core::log::init("warn");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = W;
        w.height = H;
    }
    app.add_plugins(RenderPlugin);
    let w = &mut app.world;
    let main = camera(DVec3::new(0.0, 1.8, 13.0), DVec3::new(0.0, 1.0, -6.0));
    let across = camera(DVec3::new(-4.0, 2.2, -14.0), DVec3::new(2.0, 1.0, 6.0));
    match layout {
        Layout::One => {
            w.spawn(main);
        }
        Layout::Split => {
            let half = UVec2::new(W / 2, H);
            w.spawn((main, Viewport::new(UVec2::ZERO, half)));
            w.spawn((
                across,
                ExtraView::window(),
                Viewport::new(UVec2::new(W / 2, 0), half),
            ));
        }
        Layout::Inset => {
            w.spawn(main);
            // A map-like view from high above, at low quality.
            let mut above = camera(DVec3::ZERO, -DVec3::Z);
            above.1 = Transform::from_xyz(0.0, 110.0, -2.0)
                .looking_at(DVec3::new(0.0, 0.0, -2.0), -Vec3::Z);
            w.spawn((
                above,
                ExtraView::window().with_quality(Scalability::preset(QualityPreset::Low)),
                Viewport::new(UVec2::new(W - 300, 20), UVec2::new(280, 280)),
            ));
        }
        Layout::Monitor => {
            w.spawn(main);
            let size = UVec2::new(512, 288);
            let picture = w
                .resource_mut::<Assets<Image>>()
                .add(Image::render_target(size));
            w.spawn((across, ExtraView::image(picture, size)));
            // A screen on two posts at the pond's edge, facing the main camera (a quad faces
            // +Z, so it looks away from the camera).
            let at = DVec3::new(-3.2, 1.9, 4.0);
            let viewer = DVec3::new(0.0, 1.8, 13.0);
            w.spawn((
                Transform::from_translation(at).looking_at(at + (at - viewer), Vec3::Y),
                MeshRenderer::new(
                    Shape::Quad { size: [3.2, 1.8] },
                    StandardMaterial {
                        unlit: true,
                        base_color_texture: Some(picture),
                        ..StandardMaterial::color(1.0, 1.0, 1.0)
                    },
                ),
            ));
            let frame = StandardMaterial::color(0.05, 0.05, 0.05).with_roughness(0.4);
            for dx in [-1.5, 1.5] {
                w.spawn((
                    Transform::from_xyz(at.x + dx, 0.6, at.z - 0.05),
                    MeshRenderer::new(
                        Shape::Cylinder {
                            radius: 0.06,
                            height: 1.2,
                            segments: 0,
                        },
                        frame,
                    ),
                ));
            }
        }
    }
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.5, -0.6, 0.6), Vec3::Y),
    ));
    // Forest floor, a pond, a stone slab and three balls.
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [600.0, 600.0],
                subdivisions: 8,
            },
            StandardMaterial::color(0.16, 0.2, 0.09)
                .with_roughness(0.95)
                .with_detail(1.0, 3.0)
                .with_detail_in_world(),
        ),
    ));
    w.spawn((
        Transform::from_xyz(0.0, 0.03, -2.0),
        MeshRenderer::new(
            Shape::Plane {
                size: [14.0, 12.0],
                subdivisions: 1,
            },
            StandardMaterial::color(0.01, 0.015, 0.012).with_roughness(0.04),
        ),
    ));
    for (x, z, color) in [
        (3.0, 6.5, (0.75, 0.2, 0.12)),
        (5.0, 4.0, (0.85, 0.65, 0.15)),
        (-5.5, -9.0, (0.15, 0.3, 0.8)),
    ] {
        w.spawn((
            Transform::from_xyz(x, 0.7, z),
            MeshRenderer::new(
                Shape::Sphere {
                    radius: 0.7,
                    segments: 48,
                },
                StandardMaterial::color(color.0, color.1, color.2).with_roughness(0.35),
            ),
        ));
    }
    // Conifers ringing the clearing, two and three deep.
    let bark = StandardMaterial::color(0.18, 0.12, 0.08).with_roughness(0.9);
    let needles = StandardMaterial::color(0.05, 0.11, 0.05).with_roughness(0.85);
    for ring in 0..3 {
        let radius = 17.0 + ring as f64 * 6.0;
        let count = 18 + ring * 6;
        for i in 0..count {
            let a = (i as f64 + 0.37 * ring as f64) / count as f64 * std::f64::consts::TAU;
            let jitter = ((i * 7 + ring * 3) % 5) as f64 * 0.8;
            let (x, z) = (
                (radius + jitter) * a.cos(),
                (radius + jitter) * a.sin() - 2.0,
            );
            let height = 16.0 + ((i * 13 + ring * 5) % 7) as f32 * 1.5;
            w.spawn((
                Transform::from_xyz(x, 2.0, z),
                MeshRenderer::new(
                    Shape::Cylinder {
                        radius: 0.35,
                        height: 4.0,
                        segments: 0,
                    },
                    bark,
                ),
            ));
            w.spawn((
                Transform::from_xyz(x, 2.5 + height as f64 * 0.5, z),
                MeshRenderer::new(
                    Shape::Cone {
                        radius: 3.2,
                        height,
                        segments: 0,
                    },
                    needles,
                ),
            ));
        }
    }
    app
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out =
        std::path::PathBuf::from(args.first().cloned().unwrap_or_else(|| "multi_view".into()));
    let prefix = args.get(1).cloned().unwrap_or_default();
    std::fs::create_dir_all(&out).expect("output directory");
    for (layout, name) in [
        (Layout::One, "one_camera"),
        (Layout::Split, "split_screen"),
        (Layout::Inset, "picture_in_picture"),
        (Layout::Monitor, "monitor"),
    ] {
        let mut app = clearing(layout);
        app.update_n(8);
        let path = out.join(format!("{prefix}multi_view_{name}.png"));
        capture_now(&mut app.world, &path).expect("screenshot");
        let views = app
            .world
            .resource::<Diagnostics>()
            .get("render.extra_views")
            .unwrap_or(0.0);
        println!("{} ({views} extra views)", path.display());
    }
}
