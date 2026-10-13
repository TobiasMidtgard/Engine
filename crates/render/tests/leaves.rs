//! Thin sheets (leaf cards: double-sided or alpha-masked materials) seen from the side away
//! from the sun: sunlight shines through them by `subsurface`, tinted by the leaf, and never
//! lights up or glints off the side in view, whatever their vertex normals say. Needs a GPU or
//! lavapipe; passes with a note when neither is available.

use engine_assets::Assets;
use engine_core::prelude::*;
use engine_render::mesh::{Mesh, Vertex};
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use image::RgbaImage;

const W: u32 = 320;
const H: u32 = 180;
/// Cards stand 5 m in front of the camera, 1.2 m wide, at these x positions.
const CARD_X: [f32; 3] = [-1.6, 0.0, 1.6];
const CARD_SIZE: f32 = 1.2;

/// Direction toward the sun: behind the cards (-z) and to the right, high enough to stay out
/// of the picture.
fn toward_sun() -> Vec3 {
    Vec3::new(0.7, 0.45, -0.55).normalize()
}

/// How a card is built.
#[derive(Clone, Copy)]
enum Card {
    /// One quad facing the camera (+z), double-sided material, with these vertex normals.
    DoubleSided(Vec3),
    /// A quad per side, each wound toward its own side, single-sided alpha-masked material:
    /// how foliage builds leaf cards. `front` is the normal of the side facing the camera.
    TwoQuads { front: Vec3, back: Vec3 },
}

fn card_mesh(card: Card) -> Mesh {
    let h = CARD_SIZE * 0.5;
    let corners = [(-h, -h), (h, -h), (h, h), (-h, h)];
    let quad = |normal: Vec3, base: u32, toward_camera: bool| {
        let vertices: Vec<Vertex> = corners
            .iter()
            .map(|&(x, y)| {
                Vertex::new(
                    Vec3::new(x, y, 0.0),
                    normal,
                    Vec2::new(x / CARD_SIZE + 0.5, 0.5 - y / CARD_SIZE),
                )
            })
            .collect();
        let indices = if toward_camera {
            vec![base, base + 1, base + 2, base, base + 2, base + 3]
        } else {
            vec![base, base + 2, base + 1, base, base + 3, base + 2]
        };
        (vertices, indices)
    };
    match card {
        Card::DoubleSided(normal) => {
            let (v, i) = quad(normal, 0, true);
            Mesh::new(v, i)
        }
        Card::TwoQuads { front, back } => {
            let (mut v, mut i) = quad(front, 0, true);
            let (v2, i2) = quad(back, 4, false);
            v.extend(v2);
            i.extend(i2);
            Mesh::new(v, i)
        }
    }
}

/// Three cards in a row against the sky, the sun behind them (or, with `sun_in_front`,
/// mirrored to the camera's side of them). Each card is green leaf material with `subsurface`
/// and `roughness`.
fn leaves_app(cards: [Card; 3], subsurface: f32, roughness: f32, sun_in_front: bool) -> App {
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
        s.bloom_intensity = 0.0;
        s.vignette = 0.0;
        s.film_grain = 0.0;
        s.sharpness = 0.0;
    }
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.0 },
            ..Camera::perspective(50.0)
        },
        Transform::from_xyz(0.0, 1.5, 5.0).looking_at(DVec3::new(0.0, 1.5, 0.0), Vec3::Y),
    ));
    let mut sun = toward_sun();
    if sun_in_front {
        sun.z = -sun.z;
    }
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at((-sun).as_dvec3(), Vec3::Y),
    ));
    for (card, x) in cards.into_iter().zip(CARD_X) {
        let mut material = StandardMaterial::color(0.09, 0.2, 0.035)
            .with_roughness(roughness)
            .with_subsurface(subsurface)
            .with_alpha_mask(0.5);
        material.double_sided = matches!(card, Card::DoubleSided(_));
        let mesh = w.resource_mut::<Assets<Mesh>>().add(card_mesh(card));
        w.spawn((
            Transform::from_xyz(x as f64, 1.5, 0.0),
            MeshRenderer::new(mesh, material),
        ));
    }
    app
}

fn render(mut app: App, name: &str) -> Option<RgbaImage> {
    app.update_n(3);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    Some(image::open(&path).expect("png").to_rgba8())
}

/// Mean color (0..255) of the middle of card `i`.
fn card_color(img: &RgbaImage, i: usize) -> Vec3 {
    // 5 m away with a 50° vertical field of view: pixels per meter.
    let ppm = H as f32 / (2.0 * 5.0 * 25f32.to_radians().tan());
    let cx = W as f32 * 0.5 + CARD_X[i] * ppm;
    let cy = H as f32 * 0.5;
    let half = (CARD_SIZE * ppm * 0.3) as i32;
    let mut sum = Vec3::ZERO;
    let mut n = 0.0;
    for y in -half..=half {
        for x in -half..=half {
            let p = img.get_pixel((cx as i32 + x) as u32, (cy as i32 + y) as u32);
            sum += Vec3::new(p[0] as f32, p[1] as f32, p[2] as f32);
            n += 1.0;
        }
    }
    sum / n
}

fn luma(c: Vec3) -> f32 {
    0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z
}

#[test]
fn sunlight_shines_through_backlit_leaves() {
    // A double-sided leaf and a foliage-style card (a quad per side, alpha-masked), both seen
    // from the side away from the sun.
    let cards = [
        Card::DoubleSided(Vec3::Z),
        Card::TwoQuads {
            front: Vec3::Z,
            back: -Vec3::Z,
        },
        Card::DoubleSided(Vec3::Z),
    ];
    let (Some(opaque), Some(translucent)) = (
        render(leaves_app(cards, 0.0, 0.8, false), "leaves_backlit_opaque"),
        render(
            leaves_app(cards, 0.6, 0.8, false),
            "leaves_backlit_translucent",
        ),
    ) else {
        return;
    };
    for i in 0..2 {
        let (a, b) = (card_color(&opaque, i), card_color(&translucent, i));
        eprintln!("card {i}: back {a:.1} opaque, {b:.1} with subsurface 0.6");
        assert!(
            luma(b) > luma(a) * 1.5 + 10.0,
            "card {i}: sunlight doesn't shine through: {b:.1} vs {a:.1}"
        );
        let gain = b - a;
        assert!(
            gain.y > gain.x && gain.y > gain.z && b.y > b.x && b.y > b.z,
            "card {i}: the light let through isn't leaf green: {a:.1} -> {b:.1}"
        );
    }
}

#[test]
fn the_dark_side_of_a_leaf_never_catches_the_sun() {
    let sun = toward_sun();
    // The half vector between the sun and the camera: a normal that mirrors the sun into the
    // camera, as a crown's bent normals or a normal map can on the side away from the sun.
    let glint = (sun + Vec3::Z).normalize();
    let cards = [
        // Correct normals: the reference.
        Card::DoubleSided(Vec3::Z),
        // Vertex normals that disagree with the winding (pointing at the sun).
        Card::DoubleSided(-Vec3::Z),
        // Foliage-style card whose camera-side normal is bent toward the sun.
        Card::TwoQuads {
            front: glint,
            back: (sun - Vec3::Z).normalize(),
        },
    ];
    let (Some(back), Some(front)) = (
        render(leaves_app(cards, 0.0, 0.45, false), "leaves_dark_side"),
        render(leaves_app(cards, 0.0, 0.45, true), "leaves_sunlit_side"),
    ) else {
        return;
    };
    let reference = card_color(&back, 0);
    let lit = card_color(&front, 0);
    eprintln!("reference card: {reference:.1} on its dark side, {lit:.1} sunlit");
    assert!(
        luma(lit) > luma(reference) * 2.0 + 20.0,
        "the sunlit side should be much brighter than the dark side: {lit:.1} vs {reference:.1}"
    );
    // Card 1 has the reference's geometry and must look the same; card 2's bent normal also
    // reflects the bright sky around the sun, but no sunlight.
    for (i, slack) in [(1, 1.15), (2, 1.8)] {
        let c = card_color(&back, i);
        eprintln!("card {i}: {c:.1} on its dark side");
        assert!(
            luma(c) < luma(reference) * slack + 4.0,
            "card {i} catches the sun on its dark side: {c:.1} vs {reference:.1}"
        );
    }
}
