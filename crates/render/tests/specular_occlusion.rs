//! Reflected sky is occluded by what stands around a surface: a glossy floor in a clearing ringed
//! by tall walls (a forest pond among trees, a wet courtyard) doesn't mirror the bright low sky
//! the walls hide, and a normal map's bumps don't reflect the sky from below the surface's own
//! horizon. Screen-space reflections are off, so only the environment fallback is seen. Needs a
//! GPU or lavapipe; passes with a note when neither is available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, Renderer, capture_now};
use image::RgbaImage;

const W: u32 = 320;
const H: u32 = 180;

/// Half the clearing's width, meters, and the walls' height and thickness.
const CLEARING: f64 = 15.0;
const WALL_HEIGHT: f32 = 16.0;
const WALL_THICKNESS: f32 = 4.0;

/// Points on the floor in front of the camera. Their mirror rays rise about 12° toward the far
/// wall, hitting it 3 to 5 m up.
const FLOOR_SPOTS: [DVec3; 4] = [
    DVec3::new(0.0, 0.0, 0.0),
    DVec3::new(-3.0, 0.0, -1.0),
    DVec3::new(3.0, 0.0, -1.0),
    DVec3::new(0.0, 0.0, 2.0),
];

/// Points on the open floor far ahead, seen 1 to 1.5 degrees below the horizon: their mirror
/// rays graze the floor.
const FAR_SPOTS: [DVec3; 4] = [
    DVec3::new(0.0, 0.0, -60.0),
    DVec3::new(-8.0, 0.0, -60.0),
    DVec3::new(0.0, 0.0, -90.0),
    DVec3::new(8.0, 0.0, -90.0),
];

/// A dark glossy floor (wet stone, still water) seen from 1.7 m at a low angle, with the sun
/// high behind the camera, in the open or in a clearing ringed by walls, with or without sky
/// occlusion. `floor` makes its material.
fn floor_app(
    walls: bool,
    occlusion: bool,
    floor: impl FnOnce(&mut App) -> StandardMaterial,
) -> App {
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
        s.screen_space_reflections = false;
        s.sky_occlusion = occlusion;
        s.tonemapper = Tonemapper::None;
        s.bloom_intensity = 0.0;
        s.vignette = 0.0;
        s.film_grain = 0.0;
        s.saturation = 1.0;
        s.sharpness = 0.0;
    }
    let floor = floor(&mut app);
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.0 },
            ..Camera::perspective(60.0)
        },
        Transform::from_xyz(0.0, 1.7, 8.0).looking_at(DVec3::new(0.0, 0.0, -2.0), Vec3::Y),
    ));
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.3, -0.8, -0.5), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [400.0, 400.0],
                subdivisions: 1,
            },
            floor,
        ),
    ));
    if walls {
        let stone = StandardMaterial::color(0.3, 0.3, 0.3).with_roughness(0.9);
        let reach = CLEARING + WALL_THICKNESS as f64 * 0.5;
        let long = 2.0 * CLEARING as f32 + 2.0 * WALL_THICKNESS;
        let y = WALL_HEIGHT as f64 * 0.5;
        for (center, size) in [
            (
                DVec3::new(0.0, y, -reach),
                [long, WALL_HEIGHT, WALL_THICKNESS],
            ),
            (
                DVec3::new(0.0, y, reach),
                [long, WALL_HEIGHT, WALL_THICKNESS],
            ),
            (
                DVec3::new(-reach, y, 0.0),
                [WALL_THICKNESS, WALL_HEIGHT, long],
            ),
            (
                DVec3::new(reach, y, 0.0),
                [WALL_THICKNESS, WALL_HEIGHT, long],
            ),
        ] {
            w.spawn((
                Transform::from_translation(center),
                MeshRenderer::new(Shape::Cuboid { size }, stone.clone()),
            ));
        }
    }
    app
}

/// A black glossy floor, like still water: all it shows is what it reflects.
fn glossy(_: &mut App) -> StandardMaterial {
    StandardMaterial::color(0.0, 0.0, 0.0).with_roughness(0.1)
}

fn render(app: &mut App, name: &str) -> Option<RgbaImage> {
    app.update_n(3);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    Some(image::open(&path).expect("png").to_rgba8())
}

/// sRGB-encoded value (0..255) to linear.
fn decode(v: f32) -> f32 {
    let c = v / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Mean linear luminance in a 5x5 block around where world point `p` lands on screen.
fn luminance_at(app: &App, img: &RgbaImage, p: DVec3) -> f32 {
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
            sum += 0.2126 * decode(px[0] as f32)
                + 0.7152 * decode(px[1] as f32)
                + 0.0722 * decode(px[2] as f32);
        }
    }
    sum / 25.0
}

#[test]
fn a_glossy_floor_in_a_walled_clearing_doesnt_mirror_the_low_sky() {
    let mut open = floor_app(false, true, glossy);
    let mut clearing = floor_app(true, true, glossy);
    let (Some(open_img), Some(clearing_img)) = (
        render(&mut open, "specular_occlusion_open"),
        render(&mut clearing, "specular_occlusion_clearing"),
    ) else {
        return;
    };
    for p in FLOOR_SPOTS {
        let (a, b) = (
            luminance_at(&open, &open_img, p),
            luminance_at(&clearing, &clearing_img, p),
        );
        eprintln!(
            "floor at {p}: {a:.4} in the open, {b:.4} in the clearing ({:.2})",
            b / a
        );
        // The open floor mirrors the bright low sky.
        assert!(
            a > 0.02,
            "the open floor should mirror the sky at {p} ({a:.4})"
        );
        // The walls hide that sky; what is left is the ambient fill. (Occlusion from cover
        // overhead alone leaves well over half of it: the walls hide little of the sky above.)
        assert!(
            b < 0.25 * a,
            "the floor in the clearing mirrors the low sky the walls hide at {p}: {b:.4} vs \
             {a:.4} in the open"
        );
    }
}

#[test]
fn an_open_floor_mirrors_the_sky_down_to_the_horizon() {
    // With nothing around, the floor itself (or its own bumps on the map) doesn't count as
    // cover: grazing reflections far ahead are as bright as with sky occlusion off.
    let mut with = floor_app(false, true, glossy);
    let mut without = floor_app(false, false, glossy);
    let (Some(with_img), Some(without_img)) = (
        render(&mut with, "specular_occlusion_far_on"),
        render(&mut without, "specular_occlusion_far_off"),
    ) else {
        return;
    };
    for p in FAR_SPOTS {
        let (a, b) = (
            luminance_at(&with, &with_img, p),
            luminance_at(&without, &without_img, p),
        );
        eprintln!("far floor at {p}: {a:.4} with sky occlusion, {b:.4} without");
        assert!(
            (a / b - 1.0).abs() < 0.02,
            "sky occlusion dims grazing reflections in the open at {p}: {a:.4} vs {b:.4}"
        );
    }
}

/// RGBA8 texel of a tangent-space normal.
fn encode_normal(n: Vec3) -> [u8; 4] {
    let n = n.normalize();
    let c = |v: f32| ((v * 0.5 + 0.5) * 255.0).round() as u8;
    [c(n.x), c(n.y), c(n.z), 255]
}

/// `glossy` with a strong normal map: bumps 2 m apart that tilt the normal by up to 40°.
fn bumpy_glossy(app: &mut App) -> StandardMaterial {
    let tau = std::f32::consts::TAU;
    let bumps = Image::from_fn(64, 64, false, |x, y| {
        let a = (x as f32 + 0.5) / 64.0 * tau;
        let b = (y as f32 + 0.5) / 64.0 * tau;
        encode_normal(Vec3::new(0.85 * a.sin(), 0.85 * b.cos(), 1.0))
    });
    let bumps = app
        .world
        .resource_mut::<engine_assets::Assets<Image>>()
        .add(bumps);
    // The plane's UVs are in meters.
    glossy(app)
        .with_normal_texture(bumps)
        .with_uv_scale(0.5, 0.5)
}

#[test]
fn normal_map_bumps_dont_reflect_from_below_the_horizon() {
    // At a grazing view a strong normal map tilts many reflections below the floor's own
    // horizon, into the floor itself; those fade out instead of mirroring what lies below the
    // horizon in the environment. A bumpy floor reflects much less than a flat one either way
    // (its bumps facing the camera reflect weakly), but without the fade it kept 0.92 of the
    // flat floor's reflection here.
    let mut flat = floor_app(false, true, glossy);
    let mut bumpy = floor_app(false, true, bumpy_glossy);
    let (Some(flat_img), Some(bumpy_img)) = (
        render(&mut flat, "specular_horizon_flat"),
        render(&mut bumpy, "specular_horizon_bumpy"),
    ) else {
        return;
    };
    let (mut a, mut b) = (0.0, 0.0);
    for p in FLOOR_SPOTS {
        a += luminance_at(&flat, &flat_img, p);
        b += luminance_at(&bumpy, &bumpy_img, p);
    }
    eprintln!(
        "reflection on a bumpy floor: {b:.4}, on a flat one {a:.4} ({:.3})",
        b / a
    );
    assert!(
        b < 0.5 * a,
        "the bumpy floor mirrors the environment below its horizon: {b:.4} vs {a:.4} flat"
    );
}
