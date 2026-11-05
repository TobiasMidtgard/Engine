//! Light through leaf canopies (`RenderSettings::canopy`): the ground under a layer of leaf
//! crowns sees the sky through their leaves, as the three-ring sky of crop canopy models has
//! it, and open ground sees all of it; a grey post in the shade under them, over green grass,
//! is lit a dim, nearly neutral grey, not green. Needs a GPU or lavapipe; passes with a note
//! when neither is available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{DebugView, RenderDisabled, Renderer, capture_now};
use image::RgbaImage;

const W: u32 = 320;
const H: u32 = 180;

/// Leaf cards: 8 m squares on an 8 m grid over x -64..64, z -160..0, in four layers 9-12 m up,
/// each layer a leaf area index of 1 (the cards are fully opaque).
const CARD: f32 = 8.0;
const LAYERS: [f64; 4] = [9.0, 10.0, 11.0, 12.0];

const UNDER: DVec3 = DVec3::new(0.0, 0.0, -40.0);
const OPEN: DVec3 = DVec3::new(0.0, 0.0, 40.0);
/// The grey post's foot, deep under the leaves.
const POST: DVec3 = DVec3::new(0.0, 0.0, -80.0);

fn app(canopy: bool) -> App {
    scene(canopy, true, [0.5, 0.5, 0.5])
}

/// The leaf layers over `ground`-colored ground; `sky_view` shows the sky visibility (probe GI
/// off), else the lit scene with probe GI on.
fn scene(canopy: bool, sky_view: bool, ground: [f32; 3]) -> App {
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
        if sky_view {
            s.debug_view = DebugView::SkyVisibility;
        }
        s.tonemapper = Tonemapper::None;
        s.bloom_intensity = 0.0;
        s.vignette = 0.0;
        s.film_grain = 0.0;
        s.saturation = 1.0;
        if !sky_view {
            s.anti_aliasing = AntiAliasing::None;
            s.volumetric_fog = false;
        }
        // The sky visibility on its own: probe GI's rays find the cards either way.
        s.probe_gi = !sky_view;
        s.canopy = canopy;
    }
    let w = &mut app.world;
    let eye = if sky_view {
        Transform::from_xyz(0.0, 4.0, 58.0).looking_at(DVec3::new(0.0, 0.0, -20.0), Vec3::Y)
    } else {
        Transform::from_translation(POST + DVec3::new(0.0, 3.0, 12.0))
            .looking_at(POST + DVec3::Y * 3.0, Vec3::Y)
    };
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.0 },
            ..Camera::perspective(60.0)
        },
        eye,
    ));
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(-0.2, -1.0, -0.1), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [600.0, 600.0],
                subdivisions: 1,
            },
            StandardMaterial::color(ground[0], ground[1], ground[2]).with_roughness(0.9),
        ),
    ));
    if !sky_view {
        // A grey post in the shade under the leaves, and the camera beside it.
        w.spawn((
            Transform::from_translation(POST + DVec3::Y * 3.0),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [1.0, 6.0, 1.0],
                },
                StandardMaterial::color(0.4, 0.4, 0.4).with_roughness(0.9),
            ),
        ));
    }
    let mut leaves = StandardMaterial::color(0.2, 0.5, 0.1)
        .with_alpha_mask(0.5)
        .with_wind_sway(0.05);
    leaves.double_sided = true;
    let half = CARD as f64 * 0.5;
    for &y in &LAYERS {
        for i in 0..16 {
            for k in 0..20 {
                let (x, z) = (
                    -64.0 + half + i as f64 * 8.0,
                    -160.0 + half + k as f64 * 8.0,
                );
                w.spawn((
                    Transform::from_xyz(x, y, z),
                    MeshRenderer::new(
                        Shape::Plane {
                            size: [CARD, CARD],
                            subdivisions: 1,
                        },
                        leaves,
                    ),
                ));
            }
        }
    }
    app
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

/// The debug view's value (sRGB-encoded) in a 5x5 block around where world point `p` lands.
fn value_at(app: &App, img: &RgbaImage, p: DVec3) -> f32 {
    let view = app
        .world
        .resource::<Renderer>()
        .last_view()
        .unwrap()
        .clone();
    let clip = view.view_proj * (p - view.origin).as_vec3().extend(1.0);
    let ndc = clip.truncate() / clip.w;
    let (x, y) = (
        ((ndc.x * 0.5 + 0.5) * W as f32) as i32,
        ((0.5 - ndc.y * 0.5) * H as f32) as i32,
    );
    let mut sum = 0.0;
    for dy in -2..=2 {
        for dx in -2..=2 {
            let px = img.get_pixel(
                (x + dx).clamp(0, W as i32 - 1) as u32,
                (y + dy).clamp(0, H as i32 - 1) as u32,
            );
            let c = px[1] as f32 / 255.0;
            sum += if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            };
        }
    }
    sum / 25.0
}

#[test]
fn ground_under_leaves_sees_the_sky_through_them() {
    let mut app = app(true);
    let Some(img) = render(&mut app, "canopy_sky") else {
        return;
    };
    let under = value_at(&app, &img, UNDER);
    let open = value_at(&app, &img, OPEN);
    // Leaf area index 4: 0.069 of the sky (the three-ring sky; the cards' grid and the map's
    // texels make it a little uneven).
    assert!(
        (0.035..0.14).contains(&under),
        "under the leaves the ground sees {under} of the sky, not about 0.07"
    );
    assert!(open > 0.9, "open ground sees {open} of the sky");
}

#[test]
fn the_sky_map_alone_let_most_of_the_sky_through_leaves_high_up() {
    // Without the canopy, the cards are all-or-nothing cover in the sky occlusion map, whose
    // view factor counts cover 9-12 m up over rings out to 96 m: under a closed layer the
    // ground still saw about a third of the sky (forest floors lit nearly like open ground).
    let mut off = app(false);
    let Some(img) = render(&mut off, "canopy_off_sky") else {
        return;
    };
    let without = value_at(&off, &img, UNDER);
    let mut on = app(true);
    let Some(img) = render(&mut on, "canopy_on_sky") else {
        return;
    };
    let with = value_at(&on, &img, UNDER);
    assert!(
        with < without * 0.5,
        "through the leaves the ground sees {with} of the sky, the sky map alone {without}"
    );
}

/// Linear RGB of the 9x9 block at the middle of the image (the post).
fn middle(img: &RgbaImage) -> [f32; 3] {
    let mut sum = [0.0; 3];
    for dy in -4..=4 {
        for dx in -4..=4 {
            let p = img.get_pixel((W as i32 / 2 + dx) as u32, (H as i32 / 2 + dy) as u32);
            for c in 0..3 {
                let v = p[c] as f32 / 255.0;
                sum[c] += if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                };
            }
        }
    }
    sum.map(|v| v / 81.0)
}

#[test]
fn a_grey_post_under_leaves_over_grass_is_lit_nearly_neutral() {
    // Under leaf area index 4 over sunlit-green grass: the light under a canopy is the sky
    // through its gaps and what the leaves scatter, which falls off a little slower in green
    // (`canopy_leaf_radiance`); probe GI's rays (which don't see the leaf cards) take the sun
    // through the canopy map, so the grass under the leaves isn't bounced as if sunlit. The
    // post read lime green before: G/B 3.6, most of its light the sunny grass (now 1.9).
    let mut app = scene(true, false, [0.12, 0.3, 0.06]);
    // Probe GI settles over a few frames.
    app.update_n(30);
    let Some(img) = render(&mut app, "canopy_post") else {
        return;
    };
    let [r, g, b] = middle(&img);
    eprintln!(
        "post under the leaves: {r:.4} {g:.4} {b:.4}, G/B {:.2}",
        g / b.max(1e-5)
    );
    assert!(g > 0.0005, "the post is black ({r} {g} {b})");
    assert!(
        g < b * 2.5 && g < r * 2.5,
        "the post under the leaves is lit green ({r} {g} {b})"
    );
}
