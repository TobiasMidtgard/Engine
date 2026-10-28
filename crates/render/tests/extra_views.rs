//! Extra views: a camera with an `ExtraView` renders too, besides the main camera, into its
//! rectangle of the window (picture-in-picture) or into an image a material shows (a monitor).
//! The inset looks as that camera does on its own, and the main view around it is the same as
//! without the inset; overlays that belong to the scene draw in it too, the window's UI doesn't.
//! Needs a GPU or lavapipe; passes with a note when neither is available.

use engine_assets::Assets;
use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{FrameContext, Pipelining, RenderDisabled, capture_now, wgpu};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const WINDOW: UVec2 = UVec2::new(400, 200);
/// Where the inset goes.
const INSET: Viewport = Viewport {
    position: UVec2::new(280, 10),
    size: UVec2::new(110, 70),
};

/// What the second camera does.
#[derive(Clone, Copy, PartialEq)]
enum Second {
    /// It is the main camera, in a window of the inset's size.
    Alone,
    /// There is none.
    Off,
    /// Picture-in-picture at `INSET`.
    Inset,
    /// The same at the low quality preset.
    LowInset,
    /// Into an image a screen in front of the main camera shows.
    Monitor,
}

fn app(window: UVec2, pipelining: Pipelining) -> App {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = window.x;
        w.height = window.y;
    }
    app.add_plugins(RenderPlugin);
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        s.anti_aliasing = AntiAliasing::None;
        s.film_grain = 0.0;
        s.pipelining = pipelining;
        // Probe GI lights only the main view, so a camera alone (the main view) and the same
        // camera as an inset would differ by the bounce light.
        s.probe_gi = false;
    }
    app
}

/// A red ball the main camera looks at and, 60 m away, a blue one for the second camera.
fn scene(w: &mut World, second: Second) {
    // Auto exposure adapts over real time, which differs between the runs compared.
    let camera = |at: DVec3, target: DVec3| {
        (
            Camera {
                exposure: Exposure::Manual { ev100: 14.0 },
                ..Camera::perspective(60.0)
            },
            Transform::from_translation(at).looking_at(target, Vec3::Y),
        )
    };
    let blue_camera = camera(DVec3::new(60.0, 1.5, 4.0), DVec3::new(60.0, 0.8, 0.0));
    match second {
        Second::Alone => {
            w.spawn(blue_camera);
        }
        Second::Off | Second::Inset | Second::LowInset => {
            w.spawn(camera(DVec3::new(0.0, 1.5, 4.0), DVec3::new(0.0, 0.8, 0.0)));
            let low = engine_world::Scalability::preset(engine_world::QualityPreset::Low);
            match second {
                Second::Inset => {
                    w.spawn((blue_camera, ExtraView::window(), INSET));
                }
                Second::LowInset => {
                    w.spawn((blue_camera, ExtraView::window().with_quality(low), INSET));
                }
                _ => {}
            }
        }
        Second::Monitor => {
            let picture = w
                .resource_mut::<Assets<Image>>()
                .add(Image::render_target(UVec2::new(128, 128)));
            // The main camera looks straight down at a screen showing the picture, away from the
            // red ball.
            let screen = DVec3::new(-30.0, 0.05, 0.0);
            w.spawn((
                Camera {
                    exposure: Exposure::Manual { ev100: 14.0 },
                    ..Camera::perspective(60.0)
                },
                Transform::from_translation(screen + DVec3::Y * 3.0).looking_at(screen, -Vec3::Z),
            ));
            w.spawn((
                Transform::from_translation(screen).with_scale(Vec3::splat(4.0)),
                MeshRenderer::new(
                    Shape::Plane {
                        size: [1.0, 1.0],
                        subdivisions: 1,
                    },
                    StandardMaterial {
                        unlit: true,
                        base_color_texture: Some(picture),
                        ..StandardMaterial::color(1.0, 1.0, 1.0)
                    },
                ),
            ));
            w.spawn((blue_camera, ExtraView::image(picture, UVec2::new(128, 128))));
        }
    }
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.4, -0.8, -0.45), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [200.0, 200.0],
                subdivisions: 1,
            },
            StandardMaterial::color(0.35, 0.4, 0.3),
        ),
    ));
    for (x, color) in [(0.0, (0.8, 0.25, 0.2)), (60.0, (0.15, 0.25, 0.85))] {
        w.spawn((
            Transform::from_xyz(x, 0.8, 0.0),
            MeshRenderer::new(
                Shape::Sphere {
                    radius: 0.8,
                    segments: 48,
                },
                StandardMaterial::color(color.0, color.1, color.2),
            ),
        ));
    }
}

/// The picture after a few frames, and how many extra views rendered.
fn render(second: Second, pipelining: Pipelining, shot: &str) -> Option<(image::RgbImage, f64)> {
    let window = if second == Second::Alone {
        INSET.size
    } else {
        WINDOW
    };
    let mut app = app(window, pipelining);
    scene(&mut app.world, second);
    app.update_n(5);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let dir = std::env::temp_dir().join("engine-extra-views");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(shot);
    capture_now(&mut app.world, &path).expect("screenshot");
    if let Ok(out) = std::env::var("ENGINE_SHOTS") {
        let _ = std::fs::copy(&path, std::path::Path::new(&out).join(shot));
    }
    let views = app
        .world
        .resource::<Diagnostics>()
        .get("render.extra_views")
        .unwrap_or(0.0);
    Some((image::open(&path).expect("read the shot").to_rgb8(), views))
}

fn inside_inset(x: u32, y: u32) -> bool {
    let (p, s) = (INSET.position, INSET.size);
    (p.x..p.x + s.x).contains(&x) && (p.y..p.y + s.y).contains(&y)
}

fn picture_in_picture(pipelining: Pipelining, tag: &str) {
    let Some((main, _)) = render(Second::Off, pipelining, &format!("main_{tag}.png")) else {
        return;
    };
    let Some((alone, _)) = render(Second::Alone, pipelining, &format!("alone_{tag}.png")) else {
        return;
    };
    let Some((both, views)) = render(Second::Inset, pipelining, &format!("inset_{tag}.png")) else {
        return;
    };
    assert_eq!(views, 1.0, "render.extra_views");
    // Around the inset: the main view as without it.
    let mut outside = 0u8;
    // In it: the second camera as on its own.
    let mut inset = 0u8;
    for (x, y, p) in both.enumerate_pixels() {
        let diff = |q: &image::Rgb<u8>| p.0.iter().zip(q.0).map(|(a, b)| a.abs_diff(b)).max();
        if inside_inset(x, y) {
            let q = alone.get_pixel(x - INSET.position.x, y - INSET.position.y);
            inset = inset.max(diff(q).unwrap_or(0));
        } else {
            outside = outside.max(diff(main.get_pixel(x, y)).unwrap_or(0));
        }
    }
    eprintln!("main view differs by up to {outside}, the inset by up to {inset}");
    // A level or two of bounce light's noise.
    assert!(outside <= 2, "the main view changed by up to {outside}");
    assert!(
        inset <= 2,
        "the inset differs from the camera alone by up to {inset}"
    );
    // The inset shows the blue ball, which the main view doesn't have.
    let blue = both
        .enumerate_pixels()
        .filter(|(x, y, p)| inside_inset(*x, *y) && p.0[2] as i32 > p.0[0] as i32 + 40)
        .count();
    assert!(blue > 200, "{blue} blue pixels in the inset");
}

#[test]
fn picture_in_picture_shows_the_second_camera_over_an_unchanged_main_view() {
    picture_in_picture(Pipelining::Off, "inline");
}

#[test]
fn picture_in_picture_works_pipelined() {
    picture_in_picture(Pipelining::Always, "pipelined");
}

#[test]
fn a_low_quality_inset_shows_its_camera() {
    let Some((shot, views)) = render(Second::LowInset, Pipelining::Off, "inset_low.png") else {
        return;
    };
    assert_eq!(views, 1.0, "render.extra_views");
    let blue = shot
        .enumerate_pixels()
        .filter(|(x, y, p)| inside_inset(*x, *y) && p.0[2] as i32 > p.0[0] as i32 + 40)
        .count();
    let lit = shot
        .enumerate_pixels()
        .filter(|(x, y, p)| inside_inset(*x, *y) && p.0.iter().any(|&c| c > 40))
        .count();
    let area = (INSET.size.x * INSET.size.y) as usize;
    assert!(blue > 100, "{blue} blue pixels in the inset");
    assert!(lit * 2 > area, "only {lit} of {area} inset pixels lit");
}

#[test]
fn a_camera_renders_into_an_image_a_material_shows() {
    let Some((shot, views)) = render(Second::Monitor, Pipelining::Off, "monitor.png") else {
        return;
    };
    assert_eq!(views, 1.0, "render.extra_views");
    // The screen fills the middle of the main view with the blue ball.
    let (w, h) = shot.dimensions();
    let middle = shot.get_pixel(w / 2, h / 2).0;
    eprintln!("middle of the screen: {middle:?}");
    assert!(
        middle[2] as i32 > middle[0] as i32 + 40,
        "the screen's middle is {middle:?}, not the blue ball"
    );
    let distinct: std::collections::HashSet<_> = shot.pixels().map(|p| p.0).collect();
    assert!(
        distinct.len() > 50,
        "the screen shows a picture, not one color"
    );
}

/// An overlay that counts the views it draws in; `scene`: it belongs to the scene, so it draws in
/// extra views too, and paints its whole view green.
struct Paint {
    scene: bool,
    calls: Arc<AtomicUsize>,
}

impl RenderFeature for Paint {
    fn name(&self) -> &str {
        if self.scene {
            "scene paint"
        } else {
            "ui paint"
        }
    }

    fn overlay(&mut self, encoder: &mut wgpu::CommandEncoder, ctx: &FrameContext) {
        self.calls.fetch_add(1, Ordering::Relaxed);
        if !self.scene {
            return;
        }
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("paint"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: ctx.ldr,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::GREEN),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
    }

    fn overlay_in_extra_views(&self) -> bool {
        self.scene
    }
}

#[test]
fn scene_overlays_draw_in_extra_views_and_ui_ones_do_not() {
    let mut app = app(WINDOW, Pipelining::Off);
    scene(&mut app.world, Second::Inset);
    let (scene_calls, ui_calls) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    app.add_render_feature(Paint {
        scene: true,
        calls: scene_calls.clone(),
    });
    app.add_render_feature(Paint {
        scene: false,
        calls: ui_calls.clone(),
    });
    app.update_n(3);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return;
    }
    let path = std::env::temp_dir().join("engine-extra-views-overlays.png");
    capture_now(&mut app.world, &path).expect("screenshot");
    let shot = image::open(&path).expect("read the shot").to_rgb8();
    let middle = INSET.position + INSET.size / 2;
    let inset = shot.get_pixel(middle.x, middle.y).0;
    let main = shot.get_pixel(20, 150).0;
    // The scene overlay painted both views; the UI one ran once a frame, in the main view only.
    assert!(inset[1] > 200 && inset[0] < 50, "inset {inset:?}");
    assert!(main[1] > 200 && main[0] < 50, "main view {main:?}");
    let (scene_calls, ui_calls) = (
        scene_calls.load(Ordering::Relaxed),
        ui_calls.load(Ordering::Relaxed),
    );
    assert!(ui_calls > 0, "the UI overlay ran");
    assert_eq!(
        scene_calls,
        2 * ui_calls,
        "scene overlay calls, UI {ui_calls}"
    );
}
