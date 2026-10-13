//! Thin sheets (leaf cards: double-sided or alpha-masked materials) seen from the side away
//! from the sun: sunlight shines through them by `subsurface`, tinted by the leaf, and never
//! glints off the side in view, whatever their vertex normals say. A flat leaf's dark side
//! stays dark; cards whose normals are bent up or out (grass tufts, tree crowns shaded as one
//! volume) keep the diffuse light their normals face. Needs a GPU or lavapipe; passes with a
//! note when neither is available.

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
fn the_dark_side_of_a_leaf_never_glints() {
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
    // Card 1 has the reference's geometry and must look the same.
    let wrong = card_color(&back, 1);
    eprintln!("card 1: {wrong:.1} on its dark side");
    assert!(
        luma(wrong) < luma(reference) * 1.15 + 4.0,
        "card 1 catches the sun on its dark side: {wrong:.1} vs {reference:.1}"
    );
    // Card 2's bent normal faces the sun about as much as the sunlit reference does, so it
    // keeps that much diffuse light (and reflects the bright sky around the sun), but the sun
    // mirrored straight into the camera would be several times brighter.
    let bent = card_color(&back, 2);
    eprintln!("card 2: {bent:.1} on its dark side");
    assert!(
        luma(bent) > luma(reference) * 1.5 + 10.0,
        "card 2 lost the light its bent normal faces: {bent:.1} vs {reference:.1}"
    );
    assert!(
        luma(bent) < luma(lit) * 1.3 + 8.0,
        "card 2 glints on its dark side: {bent:.1} vs the sunlit reference {lit:.1}"
    );
}

#[test]
fn grass_cards_with_upward_normals_stay_lit_from_behind() {
    // Grass and crown cards bend their normals up or out so a tuft shades as one volume: seen
    // from the side away from the sun they must stay about as bright as from the sunlit side,
    // with no `subsurface` to shine through.
    let clump = |out: Vec3| (Vec3::Y * 0.65 + out * 0.35).normalize();
    let cards = [
        Card::DoubleSided(Vec3::Y),
        Card::TwoQuads {
            front: Vec3::Y,
            back: Vec3::Y,
        },
        // Bent out toward the sun's side of the tuft, the same seen from either side.
        Card::TwoQuads {
            front: clump(Vec3::X),
            back: clump(Vec3::X),
        },
    ];
    let (Some(back), Some(front)) = (
        render(leaves_app(cards, 0.0, 0.8, false), "grass_cards_dark_side"),
        render(leaves_app(cards, 0.0, 0.8, true), "grass_cards_sunlit_side"),
    ) else {
        return;
    };
    for i in 0..3 {
        let (away, toward) = (card_color(&back, i), card_color(&front, i));
        eprintln!("card {i}: {away:.1} seen from the side away from the sun, {toward:.1} sunlit");
        assert!(
            luma(away) > luma(toward) * 0.75,
            "card {i} goes dark seen from the side away from the sun: {away:.1} vs {toward:.1}"
        );
    }
}
