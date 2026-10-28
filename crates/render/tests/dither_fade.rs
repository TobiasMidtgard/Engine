//! `DitherFade`: a mesh fades out with a screen dither, and a pair swapping levels of detail
//! covers every pixel exactly once. Needs a GPU or lavapipe; passes with a note when neither
//! is available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use image::RgbaImage;

const W: u32 = 160;
const H: u32 = 120;

/// Boxes filling the middle of the view, lit by the sun from behind the camera, without
/// anti-aliasing (the raw dither) or any post effect.
fn app(boxes: &[([f32; 3], Option<DitherFade>)]) -> App {
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
        s.tonemapper = Tonemapper::None;
        s.bloom_intensity = 0.0;
        s.vignette = 0.0;
        s.film_grain = 0.0;
        s.sharpness = 0.0;
        s.screen_space_reflections = false;
        s.screen_space_gi = false;
    }
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.0 },
            ..Camera::perspective(40.0)
        },
        Transform::from_xyz(0.0, 0.0, 5.0).looking_at(DVec3::ZERO, Vec3::Y),
    ));
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.2, -0.3, -1.0), Vec3::Y),
    ));
    for (color, fade) in boxes {
        let material = StandardMaterial::color(color[0], color[1], color[2]);
        let mut e = w.spawn((
            Transform::default(),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [2.0, 2.0, 2.0],
                },
                material,
            ),
        ));
        if let Some(fade) = fade {
            e.insert(*fade);
        }
    }
    app
}

fn render(mut app: App, name: &str) -> Option<RgbaImage> {
    app.update_n(12);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    Some(image::open(&path).expect("png").to_rgba8())
}

/// Shares of the box's face (the middle of the image) that show red, green, and neither (the
/// sky through a hole).
fn shares(img: &RgbaImage) -> (f32, f32, f32) {
    let (mut red, mut green, mut none, mut n) = (0.0, 0.0, 0.0, 0.0);
    for y in H * 3 / 8..H * 5 / 8 {
        for x in W * 3 / 8..W * 5 / 8 {
            let p = img.get_pixel(x, y);
            let [r, g, _, _] = p.0.map(|c| c as f32);
            if r > 150.0 && r > 2.0 * g {
                red += 1.0;
            } else if g > 150.0 && g > 1.5 * r {
                green += 1.0;
            } else {
                none += 1.0;
            }
            n += 1.0;
        }
    }
    (red / n, green / n, none / n)
}

#[test]
fn a_faded_mesh_hides_its_share_of_pixels() {
    let red = [0.9, 0.02, 0.02];
    let Some(whole) = render(app(&[(red, None)]), "dither_whole") else {
        return;
    };
    let quarter = render(app(&[(red, Some(DitherFade(0.25)))]), "dither_quarter").expect("GPU");
    let gone = render(app(&[(red, Some(DitherFade(1.0)))]), "dither_gone").expect("GPU");
    let (w, q, g) = (shares(&whole).0, shares(&quarter).0, shares(&gone).0);
    eprintln!("red: {w:.3} whole, {q:.3} a quarter faded, {g:.3} gone");
    assert!(w > 0.99, "{w}");
    assert!(
        (q - 0.75).abs() < 0.06,
        "a quarter faded shows {q} of its pixels"
    );
    assert!(g < 0.01, "{g}");
}

#[test]
fn a_swapping_pair_covers_every_pixel_once() {
    // Two levels of the same box in the same place (their faces at the same depth, the hardest
    // case), 30% of the way from the red one to the green one.
    let (out, into) = DitherFade::pair(0.3);
    let Some(img) = render(
        app(&[
            ([0.9, 0.02, 0.02], Some(out)),
            ([0.02, 0.9, 0.02], Some(into)),
        ]),
        "dither_pair",
    ) else {
        return;
    };
    let (red, green, none) = shares(&img);
    eprintln!("pair at 0.3: {red:.3} red, {green:.3} green, {none:.3} neither");
    assert!(none < 0.01, "holes in the swap: {none}");
    assert!(
        (red - 0.7).abs() < 0.06 && (green - 0.3).abs() < 0.06,
        "{red} red, {green} green"
    );
}
